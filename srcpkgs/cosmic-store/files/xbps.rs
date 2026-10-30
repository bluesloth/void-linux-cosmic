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
    process::{Command, Stdio},
    sync::Arc,
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
/// xbps writes its whole progress stream to stderr; the head is just the
/// download log, the actual reason lives at the tail.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Run `<pkexec> <program> <args>` and surface a useful error.
///
/// The helper output is COLLECTED, never forwarded: forwarding xbps' progress
/// stream into the process stderr is what made Discover raise a spurious
/// "Update Issue" sheet on every successful transaction.
fn run_privileged(program: &str, args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut cmd = Command::new(PKEXEC);
    cmd.arg(program).args(args).stdin(Stdio::null());
    let output = cmd.output()?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        tail(&stdout, 20)
    } else {
        tail(&stderr, 20)
    };

    match output.status.code() {
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
        _f: Box<dyn FnMut(f32) + 'static>,
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
                run_privileged(XBPS_INSTALL, &args)
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
                run_privileged(XBPS_REMOVE, &args)
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
                run_privileged(XBPS_INSTALL, &["-Syu".to_string(), "-y".to_string()])
            }
            _ => Err(format!("xbps: unsupported operation {:?}", op.kind).into()),
        }
    }
}
