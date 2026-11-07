// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

//! [UPDATE-NOTIFIER] Background update notification for the xbps backend.
//!
//! This module runs inside a dedicated `cosmic-store --notifier` process that is
//! started at session login from `/etc/xdg/autostart`. It never creates a
//! window: it periodically runs an unprivileged `xbps-install -nSu`, and when
//! the set of pending updates changes (or it has been silent for a day) it shows
//! a desktop notification whose default action opens the store on the Updates
//! page.

use std::{
    error::Error,
    io::ErrorKind,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use cosmic::Application;
use cosmic::cosmic_config::{self, CosmicConfigEntry};

use crate::{App, CONFIG_VERSION, Config, fl};

/// Mirrors the default of `Config::update_check_interval_minutes`.
const DEFAULT_INTERVAL_MINUTES: u64 = 60;
/// Never poll more often than this, whatever the config says.
const MIN_INTERVAL_MINUTES: u64 = 5;
/// How often the notifier looks at the clock and the repodata files.
const TICK: Duration = Duration::from_secs(60);
/// Repodata changes are only acted on at most this often.
const REPODATA_DEBOUNCE: Duration = Duration::from_secs(5 * 60);
/// Re-announce an unchanged update set once a day.
const RENOTIFY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// Keep the probe from wedging the notifier behind a running transaction.
const PROBE_TIMEOUT_SECS: u64 = 300;
/// How many package names to print before summarising the rest.
const MAX_BODY_PACKAGES: usize = 3;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Runs the background notifier until the user disables update checking.
pub fn run() -> Result<(), Box<dyn Error>> {
    let mut interval = match interval_minutes() {
        Some(minutes) => minutes,
        None => {
            log::info!("update-notifier: update checking is disabled (interval 0)");
            return Ok(());
        }
    };

    let state_path = state_path();
    let mut state = state_path.as_deref().map(read_state).unwrap_or_default();

    // Check right away: a login is the most likely moment for a user to act on
    // the notice, and it keeps the first notification prompt after a boot.
    let mut next_check = Instant::now();
    let mut repodata_seen = repodata_signature();
    let mut last_repodata_check = Instant::now();

    log::info!("update-notifier: starting, interval {interval} minute(s)");

    loop {
        // Honour config changes at runtime: disabling update checking stops the
        // notifier on its next tick.
        match interval_minutes() {
            Some(minutes) => interval = minutes,
            None => {
                log::info!("update-notifier: update checking was disabled, exiting");
                return Ok(());
            }
        }

        let now = Instant::now();
        let mut due = now >= next_check;
        let mut repodata_due = false;

        let signature = repodata_signature();
        if signature != repodata_seen {
            repodata_seen = signature;
            if last_repodata_check.elapsed() >= REPODATA_DEBOUNCE {
                due = true;
                repodata_due = true;
            }
        }

        if due {
            let started = Instant::now();
            // Advance the deadline even when the probe fails, so a broken or
            // locked xbps retries on the next interval rather than every tick.
            next_check = started + Duration::from_secs(interval.saturating_mul(60));
            if repodata_due {
                last_repodata_check = started;
            }
            check(&mut state, state_path.as_deref());
        }

        thread::sleep(TICK);
    }
}

/// Reads `update_check_interval_minutes`, `None` when checking is disabled.
fn interval_minutes() -> Option<u64> {
    let config = match cosmic_config::Config::new(App::APP_ID, CONFIG_VERSION) {
        Ok(handler) => match Config::get_entry(&handler) {
            Ok(config) => config,
            Err((errs, config)) => {
                log::info!("update-notifier: errors loading config: {:?}", errs);
                config
            }
        },
        Err(err) => {
            log::warn!("update-notifier: failed to create config handler: {err}");
            return Some(DEFAULT_INTERVAL_MINUTES);
        }
    };

    if config.update_check_interval_minutes == 0 {
        None
    } else {
        Some(config.update_check_interval_minutes.max(MIN_INTERVAL_MINUTES))
    }
}

/// One probe: returns the sorted set of packages with a pending update.
fn probe() -> Result<Vec<String>, Box<dyn Error>> {
    let xbps_install = xbps_install();
    let output = match Command::new("timeout")
        .arg(PROBE_TIMEOUT_SECS.to_string())
        .arg(&xbps_install)
        .arg("-nSu")
        .output()
    {
        Ok(output) => output,
        // No `timeout` on this system: run the probe directly.
        Err(err) if err.kind() == ErrorKind::NotFound => {
            Command::new(&xbps_install).arg("-nSu").output()?
        }
        Err(err) => return Err(err.into()),
    };

    if !output.status.success() {
        return Err(format!(
            "xbps-install -nSu failed ({})",
            describe_status(output.status)
        )
        .into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = Vec::new();
    for line in stdout.lines() {
        // `<pkgver> <action> <arch> <repo> <installed size> <download size>`
        let mut fields = line.split_whitespace();
        if let (Some(pkgver), Some(action)) = (fields.next(), fields.next())
            && action == "update"
        {
            packages.push(pkgver.to_string());
        }
    }

    packages.sort_unstable();
    packages.dedup();
    Ok(packages)
}

/// Prefers the real binary so the `void-xbps-snap` shadow is not involved.
fn xbps_install() -> PathBuf {
    let real = Path::new("/usr/bin/xbps-install.real");
    if real.exists() {
        real.to_path_buf()
    } else {
        PathBuf::from("/usr/bin/xbps-install")
    }
}

fn describe_status(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        format!("exit {code}")
    } else if let Some(signal) = status.signal() {
        format!("terminated by signal {signal}")
    } else {
        "terminated abnormally".to_string()
    }
}

/// Probes once and notifies only when there is something new to say.
fn check(state: &mut NotifierState, state_path: Option<&Path>) {
    let packages = match probe() {
        Ok(packages) => packages,
        Err(err) => {
            log::warn!("update-notifier: update probe failed: {err}");
            return;
        }
    };

    if packages.is_empty() {
        // A successful empty probe clears the remembered set, so the next real
        // update is always announced again.
        if *state != NotifierState::default() {
            log::info!("update-notifier: no pending updates, clearing state");
            *state = NotifierState::default();
            persist(state_path, *state);
        }
        return;
    }

    let fingerprint = fingerprint(&packages);
    let now = unix_now();
    let changed = state.fingerprint != fingerprint;
    let stale = now.saturating_sub(state.notified_at) >= RENOTIFY_AFTER.as_secs();
    log::info!(
        "update-notifier: {} pending update(s), changed={changed}, stale={stale}",
        packages.len()
    );

    if !changed && !stale {
        return;
    }

    match notify(&packages) {
        Ok(()) => {
            *state = NotifierState {
                fingerprint,
                notified_at: now,
            };
            persist(state_path, *state);
        }
        Err(err) => log::warn!("update-notifier: failed to show notification: {err}"),
    }
}

/// Shows the notification and routes its default action to the Updates page.
fn notify(packages: &[String]) -> Result<(), Box<dyn Error>> {
    let count = packages.len();
    let summary = fl!("update-notifier-summary", count = count);
    let body = if count > MAX_BODY_PACKAGES {
        let shown = packages[..MAX_BODY_PACKAGES].join(", ");
        // The remainder is bound to a typed local first: `i18n-embed-fl` passes
        // each named argument through `Into<FluentValue>`, and an inline
        // subtraction leaves `Sub::Output` uninferable (E0284).
        let remaining: usize = count - MAX_BODY_PACKAGES;
        fl!(
            "update-notifier-more",
            packages = shown.as_str(),
            count = remaining
        )
    } else {
        packages.join(", ")
    };
    let action_label = fl!("update-notifier-action");

    let mut notification = notify_rust::Notification::new();
    notification
        .appname("cosmic-store")
        .summary(&summary)
        .body(&body)
        .auto_icon()
        .action("default", &action_label)
        .timeout(notify_rust::Timeout::Never);

    // `NotificationHandle` is not `Send`, so it is created and consumed on the
    // same thread; only the result of `show()` comes back to the caller.
    let (shown_tx, shown_rx) = mpsc::channel();
    thread::spawn(move || match notification.show() {
        Ok(handle) => {
            let _ = shown_tx.send(Ok(()));
            handle.wait_for_action(|action| {
                if action == "default"
                    && let Ok(exe) = std::env::current_exe()
                {
                    let _ = Command::new(exe).arg("--updates").spawn();
                }
            });
        }
        Err(err) => {
            let _ = shown_tx.send(Err(err.to_string()));
        }
    });

    match shown_rx.recv() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(err) => Err(err.into()),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NotifierState {
    /// Fingerprint of the last announced update set; `0` means "none".
    fingerprint: u64,
    /// Unix seconds of the last announcement.
    notified_at: u64,
}

fn state_path() -> Option<PathBuf> {
    dirs::state_dir().map(|dir| dir.join("cosmic-store").join("update-notifier.state"))
}

fn read_state(path: &Path) -> NotifierState {
    let mut state = NotifierState::default();
    let Ok(contents) = std::fs::read_to_string(path) else {
        return state;
    };

    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<u64>() else {
            continue;
        };
        match key.trim() {
            "fingerprint" => state.fingerprint = value,
            "notified_at" => state.notified_at = value,
            _ => {}
        }
    }
    state
}

fn write_state(path: &Path, state: NotifierState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        format!(
            "fingerprint={}\nnotified_at={}\n",
            state.fingerprint, state.notified_at
        ),
    )
}

fn persist(path: Option<&Path>, state: NotifierState) {
    let Some(path) = path else {
        log::debug!("update-notifier: no state directory available");
        return;
    };
    if let Err(err) = write_state(path, state) {
        log::warn!("update-notifier: failed to write state: {err}");
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0)
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

/// Stable fingerprint of an update set; `0` is reserved for "no updates".
fn fingerprint(packages: &[String]) -> u64 {
    let mut hash = FNV_OFFSET;
    for package in packages {
        fnv1a(&mut hash, package.as_bytes());
        fnv1a(&mut hash, b"\n");
    }
    hash.max(1)
}

/// Signature of the cached repodata files, used to notice a repository sync.
fn repodata_signature() -> u64 {
    let Ok(dir) = std::fs::read_dir("/var/db/xbps") else {
        return 0;
    };

    let mut entries = Vec::new();
    for entry in dir.flatten() {
        let path = entry.path().join("x86_64-repodata");
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|delta| delta.as_secs())
            .unwrap_or(0);
        entries.push(format!("{}:{modified}:{}", path.display(), metadata.len()));
    }
    entries.sort_unstable();

    let mut hash = FNV_OFFSET;
    for entry in &entries {
        fnv1a(&mut hash, entry.as_bytes());
        fnv1a(&mut hash, b"\n");
    }
    hash
}
