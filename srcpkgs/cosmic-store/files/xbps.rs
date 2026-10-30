//! Native XBPS backend for the COSMIC Store.
//!
//! Void Linux does not ship any AppStream catalogs, so a plain
//! `AppstreamCache` for this source would always be empty. Instead we
//! synthesize one `AppInfo` per package straight from the xbps CLI:
//!
//!   * `xbps-query -l`   -> installed packages (authoritative version)
//!   * `xbps-query -Rs ""` -> everything available in the configured repos
//!
//! The result is rebuilt on every launch and deliberately NOT persisted to
//! the on-disk appstream cache: a package installed or removed behind the
//! store's back (xbps CLI, OctoXBPS, another session) shows up immediately,
//! packages that disappeared from the repos stop being offered, and there is
//! no cache-invalidation bug to get wrong. A full rebuild costs ~0.4 s.
//!
//! Privileged operations go through `pkexec` with a dedicated polkit action
//! (`org.freedesktop.policykit.pkexec.cosmic-store.xbps.*`, `auth_admin`,
//! see the carried `.policy` file). House rule: never emit a bare `-Sy` /
//! `-Syy` -- OctoXBPS ships a NOPASS polkit action keyed on `argv1 == -Sy`,
//! so a bare sync would be passwordless. We use `-y` for transactions,
//! `-R` for removals (so dependencies are not orphaned) and `-Syu` for the
//! system update.

use std::{
    collections::HashMap,
    error::Error,
    io::Read,
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    thread,
};

use super::{Backend, Package};
use crate::{
    AppId, AppInfo, AppstreamCache, Operation, OperationKind,
    app_info::{AppIcon, AppKind, AppRelease},
};

/// Program used to run a privileged xbps transaction.
const PKEXEC: &str = "pkexec";
/// Absolute path to `xbps-install`.
///
/// The absolute path is load-bearing: pkexec matches a policy action's
/// `org.freedesktop.policykit.exec.path` annotation against the path it
/// actually resolves and executes. A bare name is resolved through PATH,
/// and on this box `/usr/sbin` precedes `/usr/bin` in the environment the
/// store inherits, so `pkexec xbps-install` becomes
/// `/usr/sbin/xbps-install` — which no action is annotated for. pkexec then
/// silently falls back to its generic `org.freedesktop.policykit.exec`
/// action and this backend's dedicated policy is never consulted (the dialog
/// asks for `/usr/sbin/xbps-install` with pkexec's stock wording).
const XBPS_INSTALL: &str = "/usr/bin/xbps-install";
/// Absolute path to `xbps-remove` (same reasoning as [`XBPS_INSTALL`]).
const XBPS_REMOVE: &str = "/usr/bin/xbps-remove";
/// Icon used for synthesized packages until a real one can be resolved.
const STOCK_ICON: &str = "package-x-generic";
/// Prefix that marks a synthesized AppStream id as belonging to xbps.
const ID_PREFIX: &str = "xbps:";

#[derive(Debug)]
pub struct Xbps {
    appstream_caches: Vec<AppstreamCache>,
}

impl Xbps {
    pub fn new(locale: &str) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            appstream_caches: vec![AppstreamCache::system(
                "xbps".to_string(),
                "Void Linux".to_string(),
                locale,
            )],
        })
    }
}

/// Split an xbps `pkgver` (`name-version_revision`) at the LAST dash.
///
/// Package names may themselves contain dashes (`libglib-devel`), while the
/// version/revision part never does, so the last dash is the only correct
/// split point.
fn split_name_version(pkgver: &str) -> (String, String) {
    if let Some(pos) = pkgver.rfind('-') {
        let (name, ver) = pkgver.split_at(pos);
        let ver = &ver[1..];
        if !name.is_empty() && !ver.is_empty() {
            return (name.to_string(), ver.to_string());
        }
    }
    (pkgver.to_string(), String::new())
}

/// Run a command, capturing both streams. Unprivileged queries only.
fn run_cmd(cmd: &str, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{} {:?} failed: {}", cmd, args, stderr.trim()).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Last `n` lines of a captured stream, for error messages.
///
/// xbps writes its progress lines to stdout when stdout is not a tty, and its
/// errors to stderr; the head of either is the download log, the actual reason
/// lives at the tail.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Parse a size rendered by xbps' `humanize_number` (`9981B`, `36KB`, `918GB`)
/// back into a byte count.
fn humanize_to_bytes(s: &str) -> Option<f64> {
    let s = s.trim();
    let (mut digits, mut unit) = (String::new(), String::new());
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' {
            if !unit.is_empty() {
                return None;
            }
            digits.push(c);
        } else if c.is_ascii_alphabetic() {
            unit.push(c.to_ascii_uppercase());
        } else {
            return None;
        }
    }
    let value: f64 = digits.parse().ok()?;
    let scale = match unit.as_str() {
        "" | "B" => 1.0,
        "K" | "KB" => 1024.0,
        "M" | "MB" => 1024.0 * 1024.0,
        "G" | "GB" => 1024.0 * 1024.0 * 1024.0,
        "T" | "TB" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some(value * scale)
}

/// Pull the count out of a plan line, which `transaction.c` renders as
/// `N packages will be <what>:` (`1 package will be removed:`).
fn plan_count(line: &str, what: &str) -> Option<u32> {
    let suffix = format!("will be {what}:");
    let head = line.trim().strip_suffix(&suffix)?.trim();
    let head = head
        .strip_suffix("packages")
        .or_else(|| head.strip_suffix("package"))?
        .trim();
    head.parse().ok()
}

/// Split `<pkgver>: <rest>` on the first `": "` (pkgvers never contain one).
fn split_pkg_line(line: &str) -> Option<&str> {
    line.split_once(": ").map(|(_, rest)| rest)
}

/// Stage of a transaction, as announced by its `[*] ...` headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Prepare,
    Download,
    Verify,
    Unpack,
    Configure,
    Done,
}

impl Phase {
    fn from_header(header: &str) -> Option<Self> {
        let header = header.trim();
        if header.starts_with("Updating repository") {
            Some(Self::Prepare)
        } else if header.starts_with("Downloading packages") {
            Some(Self::Download)
        } else if header.starts_with("Verifying package integrity")
            || header.starts_with("Collecting package files")
        {
            Some(Self::Verify)
        } else if header.starts_with("Unpacking packages") {
            Some(Self::Unpack)
        } else if header.starts_with("Configuring unpacked packages") {
            Some(Self::Configure)
        } else {
            None
        }
    }
}

/// Turns xbps' output stream into a monotonically increasing 0..=100 value.
///
/// xbps never reports a percentage of its own, so the estimate is
/// phase-weighted:
///
///   * plan / repo sync          1
///   * download (by bytes)       1 .. 40
///   * verify + collect files    40
///   * unpack (by package)       40 .. 85
///   * configure (by package)    85 .. 99
///   * success                   100
///
/// The denominators come from the transaction plan (`N packages will be ...`)
/// plus `Size to download:`, and the download leg is refined by the per-file
/// percentages xbps prints once a second while stdout is a pipe.
#[derive(Debug)]
struct ProgressTracker {
    phase: Phase,
    planned_downloads: u32,
    planned_transactions: u32,
    planned_removals: u32,
    total_download_bytes: f64,
    downloaded_files: u32,
    bytes_done: f64,
    bytes_in_flight: f64,
    unpacked: u32,
    configured: u32,
    removed: u32,
    last: f32,
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self {
            phase: Phase::Prepare,
            planned_downloads: 0,
            planned_transactions: 0,
            planned_removals: 0,
            total_download_bytes: 0.0,
            downloaded_files: 0,
            bytes_done: 0.0,
            bytes_in_flight: 0.0,
            unpacked: 0,
            configured: 0,
            removed: 0,
            last: 0.0,
        }
    }
}

impl ProgressTracker {
    fn observe(&mut self, line: &str) {
        let line = line.trim_end();

        // Phase headers: `[*] Downloading packages`.
        if let Some(header) = line.strip_prefix("[*] ") {
            if let Some(phase) = Phase::from_header(header) {
                // Never rewind: xbps announces phases in order, but a re-sync
                // or an unexpected message must not move the bar backwards.
                if phase > self.phase {
                    self.phase = phase;
                }
            }
            return;
        }

        // Transaction plan.
        if let Some(n) = plan_count(line, "downloaded") {
            self.planned_downloads = n;
            return;
        }
        if let Some(n) = plan_count(line, "installed") {
            self.planned_transactions += n;
            return;
        }
        if let Some(n) = plan_count(line, "updated") {
            self.planned_transactions += n;
            return;
        }
        if let Some(n) = plan_count(line, "removed") {
            self.planned_removals = n;
            return;
        }
        if let Some(value) = line.trim().strip_prefix("Size to download:") {
            if let Some(bytes) = humanize_to_bytes(value) {
                self.total_download_bytes = bytes;
            }
            return;
        }

        if self.phase == Phase::Download {
            if let Some(rest) = split_pkg_line(line) {
                // In flight: `<file>: [<size> <pct>%] <rate> ETA: <eta>`.
                if let Some(inner) = rest.strip_prefix('[') {
                    if let Some((payload, _)) = inner.split_once(']') {
                        let parts: Vec<&str> = payload.split_whitespace().collect();
                        if let (Some(size), Some(pct)) = (
                            parts.first().and_then(|s| humanize_to_bytes(s)),
                            parts
                                .get(1)
                                .and_then(|p| p.strip_suffix('%'))
                                .and_then(|p| p.parse::<f64>().ok()),
                        ) {
                            self.bytes_in_flight = (size * pct / 100.0).clamp(0.0, size);
                        }
                        return;
                    }
                }
                // Completed: `<file>: <size> [avg rate: <rate>]`.
                if let Some((size, _)) = rest.split_once(" [avg rate: ") {
                    if let Some(bytes) = humanize_to_bytes(size) {
                        self.bytes_done += bytes;
                        self.bytes_in_flight = 0.0;
                        if !line.contains(".sig2") {
                            self.downloaded_files += 1;
                        }
                    }
                    return;
                }
            }
        }

        // Per-package events. `installed ...`/`updated ...` also print
        // `configuring ...`, so this counts the latter only.
        if line.ends_with(": unpacking ...") || line.contains(": updating to ") {
            self.unpacked += 1;
        } else if line.ends_with(": configuring ...") {
            self.configured += 1;
        } else if line.ends_with(": removed successfully.")
            // `xbps-remove` words it differently: `Removed `pkg-1_1' successfully.`
            || (line.starts_with("Removed `") && line.ends_with("' successfully."))
        {
            self.removed += 1;
        }
    }

    fn percent(&mut self) -> f32 {
        let value = match self.phase {
            // A pure removal (`xbps-remove`) announces no phase headers at
            // all, so its whole run is reported from the plan instead.
            Phase::Prepare if self.planned_removals > 0 => {
                1.0 + 98.0 * self.fraction(self.removed, self.planned_removals)
            }
            Phase::Prepare => 1.0,
            Phase::Download => {
                let ratio = if self.total_download_bytes > 0.0 {
                    (self.bytes_done + self.bytes_in_flight) / self.total_download_bytes
                } else if self.planned_downloads > 0 {
                    self.downloaded_files as f64 / self.planned_downloads as f64
                } else {
                    0.0
                };
                1.0 + 39.0 * ratio.clamp(0.0, 1.0) as f32
            }
            Phase::Verify => 40.0,
            Phase::Unpack => 40.0 + 45.0 * self.fraction(self.unpacked, self.planned_transactions),
            Phase::Configure => {
                let done = if self.planned_transactions > 0 {
                    self.fraction(self.configured, self.planned_transactions)
                } else {
                    self.fraction(self.removed, self.planned_removals)
                };
                85.0 + 14.0 * done
            }
            Phase::Done => 100.0,
        };
        self.last = value.max(self.last);
        self.last
    }

    /// Fraction complete, or the start of the phase when there is no plan to
    /// compare against (never invent movement).
    fn fraction(&self, done: u32, total: u32) -> f32 {
        if total == 0 {
            0.0
        } else {
            (done as f32 / total as f32).clamp(0.0, 1.0)
        }
    }
}

/// Drain one child stream, splitting on `\n` and `\r` (xbps uses a bare `\r`
/// for its tty progress line) and forwarding each fragment with a flag saying
/// whether it came from stderr.
fn pump<R>(mut stream: R, tx: mpsc::Sender<(bool, String)>, is_stderr: bool)
where
    R: Read,
{
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                for &byte in &chunk[..n] {
                    if byte == b'\n' || byte == b'\r' {
                        if !buf.is_empty() {
                            let line = String::from_utf8_lossy(&buf).into_owned();
                            buf.clear();
                            if tx.send((is_stderr, line)).is_err() {
                                return;
                            }
                        }
                    } else if buf.len() < 8192 {
                        buf.push(byte);
                    }
                }
            }
            Err(ref err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if !buf.is_empty() {
        let _ = tx.send((is_stderr, String::from_utf8_lossy(&buf).into_owned()));
    }
}

/// Run `<pkexec> <program> <args>`, streaming its output into `progress` and
/// surfacing a useful error.
///
/// The helper output is COLLECTED, never forwarded: forwarding xbps' progress
/// stream into the process' own stderr is what made Discover raise a spurious
/// "Update Issue" sheet on every successful transaction. Collecting it line by
/// line still gives the store a live progress callback, and keeps the tail
/// available for the error message.
fn run_privileged<F>(program: &str, args: &[String], mut progress: F) -> Result<(), Box<dyn Error>>
where
    F: FnMut(f32),
{
    let mut cmd = Command::new(PKEXEC);
    cmd.arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    progress(0.0);
    let mut child = cmd.spawn()?;

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    let mut pumps = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        pumps.push(thread::spawn(move || pump(stdout, tx, false)));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        pumps.push(thread::spawn(move || pump(stderr, tx, true)));
    }
    drop(tx);

    let mut tracker = ProgressTracker::default();
    let mut stdout_log: Vec<String> = Vec::new();
    let mut stderr_log: Vec<String> = Vec::new();

    while let Ok((is_stderr, line)) = rx.recv() {
        tracker.observe(&line);
        progress(tracker.percent());
        if is_stderr {
            stderr_log.push(line);
        } else {
            stdout_log.push(line);
        }
    }
    for handle in pumps {
        let _ = handle.join();
    }
    let status = child.wait()?;

    if status.success() {
        progress(100.0);
        return Ok(());
    }

    let stderr = stderr_log.join("\n");
    let stdout = stdout_log.join("\n");
    let detail = if stderr.trim().is_empty() {
        tail(&stdout, 20)
    } else {
        tail(&stderr, 20)
    };

    match status.code() {
        // pkexec: the user dismissed the polkit dialog.
        Some(126) => Err("Authentication was cancelled".into()),
        // pkexec: not authorized, or the helper could not be executed.
        Some(127) => Err(format!("pkexec could not run {program}").into()),
        code => {
            if detail.is_empty() {
                Err(format!("{program} failed (exit {code:?})").into())
            } else {
                Err(format!("{program} failed (exit {code:?}):\n{detail}").into())
            }
        }
    }
}

/// Insert one synthesized package into the cache.
///
/// Returns `true` if a new entry was created. Installed packages are offered
/// first so they win over the (possibly newer) repo copy: their version is
/// what is actually on the system.
fn insert_package(
    cache: &mut AppstreamCache,
    source_id: &str,
    source_name: &str,
    pkgver: &str,
    summary: &str,
) -> bool {
    let (name, version) = split_name_version(pkgver);
    if name.is_empty() || name.starts_with('.') {
        return false;
    }
    // Already present (installed pass ran first, or a duplicate repo entry).
    if cache.pkgnames.contains_key(&name) {
        return false;
    }

    let app_id = AppId::new(&format!("{ID_PREFIX}{name}"));
    if cache.infos.contains_key(&app_id) {
        return false;
    }

    let summary = if summary.is_empty() {
        name.clone()
    } else {
        summary.to_string()
    };

    let info = Arc::new(AppInfo {
        source_id: source_id.to_string(),
        source_name: source_name.to_string(),
        origin_opt: None,
        name: name.clone(),
        summary: summary.clone(),
        kind: AppKind::DesktopApplication,
        developer_name: "Void Linux".to_string(),
        description: summary,
        license_opt: None,
        pkgnames: vec![name.clone()],
        package_paths: Vec::new(),
        categories: Vec::new(),
        desktop_ids: Vec::new(),
        flatpak_refs: Vec::new(),
        icons: vec![AppIcon::Stock(STOCK_ICON.to_string())],
        provides: Vec::new(),
        releases: vec![AppRelease {
            timestamp: None,
            version,
            description: None,
            url: None,
        }],
        screenshots: Vec::new(),
        urls: Vec::new(),
        monthly_downloads: 0,
    });

    cache
        .pkgnames
        .entry(name)
        .or_default()
        .insert(app_id.clone());
    cache.infos.insert(app_id, info);
    true
}

/// Walk an `xbps-query -l` / `-Rs` listing, calling `add` per line.
///
/// Both formats are `<state> <pkgver> <description...>` where the state is
/// `ii` for installed and `[-]` / `[ii]` for repo query results.
fn each_line<F>(output: &str, mut add: F) -> usize
where
    F: FnMut(&str, &str) -> bool,
{
    let mut added = 0;
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        // State marker.
        if parts.next().is_none() {
            continue;
        }
        let pkgver = match parts.next() {
            Some(pkgver) => pkgver,
            None => continue,
        };
        let summary = parts.collect::<Vec<_>>().join(" ");
        if add(pkgver, &summary) {
            added += 1;
        }
    }
    added
}

impl Xbps {
    fn synthesize_apps(&mut self) {
        let cache = &mut self.appstream_caches[0];
        let source_id = cache.source_id.clone();
        let source_name = cache.source_name.clone();

        // Rebuild from scratch: never merge into a stale cache.
        cache.infos.clear();
        cache.pkgnames.clear();
        cache.addons.clear();

        let installed = match run_cmd("xbps-query", &["-l"]) {
            Ok(out) => each_line(&out, |pkgver, summary| {
                insert_package(cache, &source_id, &source_name, pkgver, summary)
            }),
            Err(err) => {
                log::warn!("xbps: could not list installed packages: {err}");
                0
            }
        };

        let available = match run_cmd("xbps-query", &["-Rs", ""]) {
            Ok(out) => each_line(&out, |pkgver, summary| {
                insert_package(cache, &source_id, &source_name, pkgver, summary)
            }),
            Err(err) => {
                log::warn!("xbps: could not list available packages: {err}");
                0
            }
        };

        log::info!(
            "xbps: synthesized {} apps from {} installed and {} available packages",
            cache.infos.len(),
            installed,
            available
        );
    }
}

impl Backend for Xbps {
    fn load_caches(&mut self, _refresh: bool) -> Result<(), Box<dyn Error>> {
        // Deliberately does not touch the bitcode appstream cache: the
        // synthesized entries are rebuilt from the xbps CLI every launch.
        self.synthesize_apps();
        Ok(())
    }

    fn info_caches(&self) -> &[AppstreamCache] {
        &self.appstream_caches
    }

    fn installed(&self) -> Result<Vec<Package>, Box<dyn Error>> {
        let cache = &self.appstream_caches[0];
        let out = run_cmd("xbps-query", &["-l"])?;
        let mut packages = Vec::new();

        for line in out.lines() {
            let mut parts = line.split_whitespace();
            if parts.next().is_none() {
                continue;
            }
            let pkgver = match parts.next() {
                Some(pkgver) => pkgver,
                None => continue,
            };
            let (name, version) = split_name_version(pkgver);

            let id = match cache.pkgnames.get(&name) {
                Some(ids) => ids.iter().next().cloned(),
                None => Some(AppId::new(&format!("{ID_PREFIX}{name}"))),
            };
            let Some(id) = id else { continue };
            let Some(info) = cache.infos.get(&id) else {
                log::debug!("xbps: no app info for installed package {name}");
                continue;
            };

            packages.push(Package {
                icon: cache.icon(info),
                id,
                info: info.clone(),
                version,
                extra: HashMap::new(),
            });
        }

        Ok(packages)
    }

    fn updates(&self) -> Result<Vec<Package>, Box<dyn Error>> {
        let cache = &self.appstream_caches[0];
        // `-nSu` is a dry run and works unprivileged from the cached repodata.
        let out = match run_cmd(XBPS_INSTALL, &["-nSu"]) {
            Ok(out) => out,
            Err(err) => {
                log::debug!("xbps: update check failed: {err}");
                return Ok(Vec::new());
            }
        };

        let mut packages = Vec::new();
        for line in out.lines() {
            // <pkgver> <action> <arch> <repo> <installed size> <download size>
            let mut parts = line.split_whitespace();
            let pkgver = match parts.next() {
                Some(pkgver) => pkgver,
                None => continue,
            };
            let (name, version) = split_name_version(pkgver);

            let id = match cache.pkgnames.get(&name) {
                Some(ids) => ids.iter().next().cloned(),
                None => Some(AppId::new(&format!("{ID_PREFIX}{name}"))),
            };
            let Some(id) = id else { continue };
            let Some(info) = cache.infos.get(&id) else {
                continue;
            };

            packages.push(Package {
                icon: cache.icon(info),
                id,
                info: info.clone(),
                version,
                extra: HashMap::new(),
            });
        }

        Ok(packages)
    }

    fn file_packages(&self, _path: &str) -> Result<Vec<Package>, Box<dyn Error>> {
        // Not implemented: mapping a file back to its package needs
        // `xbps-query -o`, which is a separate feature.
        Ok(Vec::new())
    }

    fn operation(
        &self,
        op: &Operation,
        mut f: Box<dyn FnMut(f32) + 'static>,
    ) -> Result<(), Box<dyn Error>> {
        // Collect the package names the operation applies to. `infos` is the
        // normal path; `package_ids` covers ids that never got an AppInfo.
        let mut pkgs: Vec<String> = Vec::new();
        for info in &op.infos {
            for pkgname in &info.pkgnames {
                if !pkgs.contains(pkgname) {
                    pkgs.push(pkgname.clone());
                }
            }
        }
        if pkgs.is_empty() {
            for id in &op.package_ids {
                let raw = id.raw();
                let name = raw.strip_prefix(ID_PREFIX).unwrap_or(raw);
                if !name.is_empty() && !pkgs.iter().any(|p| p == name) {
                    pkgs.push(name.to_string());
                }
            }
        }

        match op.kind {
            OperationKind::Install => {
                if pkgs.is_empty() {
                    return Err("xbps: no packages to install".into());
                }
                // `pkexec xbps-install -y -- <pkgs...>`: the `--` terminator
                // appears exactly once, before the whole package list.
                let mut args = vec!["-y".to_string(), "--".to_string()];
                args.extend(pkgs);
                log::info!("xbps: install {args:?}");
                run_privileged(XBPS_INSTALL, &args, |p| f(p))
            }
            OperationKind::Uninstall { .. } => {
                if pkgs.is_empty() {
                    return Err("xbps: no packages to remove".into());
                }
                // `-R` / `--recursive` is load-bearing: a plain
                // `xbps-remove` takes only the named package, leaving every
                // dependency behind as an orphan (removing one `vlc` left 59
                // orphaned packages on the dev box), so each uninstall would
                // silently grow the orphan set. `-R` also drops dependencies
                // that no other installed package needs. Matches the GNOME
                // Software backend's `--yes --recursive`.
                //
                // The flags stay ahead of `--` and the first argument is
                // still `-y`, so the policy's `exec.argv1 = -y` annotation
                // (shared with the install action) keeps matching.
                let mut args = vec!["-y".to_string(), "-R".to_string(), "--".to_string()];
                args.extend(pkgs);
                log::info!("xbps: remove {args:?}");
                run_privileged(XBPS_REMOVE, &args, |p| f(p))
            }
            OperationKind::Update => {
                // A full sync+upgrade. `-Syu`, never a bare `-Sy`.
                //
                // `-y` is load-bearing: `run_privileged` sets stdin to
                // /dev/null, so xbps' "Do you want to continue?" prompt reads
                // EOF -- and its question() treats EOF as "no", printing
                // "Aborting!" while exec_transaction still returns 0. Without
                // `-y` the upgrade silently does nothing and reports success.
                // argv1 stays `-Syu`, so the policy annotation still matches.
                log::info!("xbps: system update");
                run_privileged(
                    XBPS_INSTALL,
                    &["-Syu".to_string(), "-y".to_string()],
                    |p| f(p),
                )
            }
            _ => Err(format!("xbps: unsupported operation {:?}", op.kind).into()),
        }
    }
}
