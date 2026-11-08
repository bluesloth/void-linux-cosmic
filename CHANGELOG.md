# Changelog

All notable changes to **Void Linux COSMIC** — the `xbps-src` packaging sources
for [COSMIC](https://github.com/pop-os/cosmic-epoch) on Void Linux *and* the
signed binary repository served from this repository's root — are documented in
this file.

## Release model

COSMIC ships as **epoch** releases: the aggregate
[`pop-os/cosmic-epoch`](https://github.com/pop-os/cosmic-epoch) repository and
every component repository carry the same `epoch-X.Y.Z` tag. We mirror that
scheme: each release of this repository is tagged **`epoch-X.Y.Z`** for the
upstream epoch it packages, so a single number identifies the whole component
set. (Upstream release pages are titled "COSMIC Epoch X.Y.Z"; ours follow.)

Each tag marks the state of *both* halves of this repository for that epoch —
the pinned templates under `srcpkgs/` and the built `.xbps` packages, their
`.sig2` signatures and the signed `<arch>-repodata` index at the root that
GitHub Pages serves at <https://bluesloth.github.io/void-linux-cosmic/>.

Tracking a new upstream epoch is `tools/sync-upstream.sh`; see
`docs/UPSTREAM.md` for the full release procedure and the special cases
(commit-pinned `libcosmic`, `pop-launcher`'s own version, the meta packages).

## epoch-1.11.0-r8 — 2026-11-07

Store release: the store now tells you when updates exist. Only `cosmic-store`
is re-cut (revision 11 → 12).

### The defect

After revision 11 the store could *see* updates, but only if you opened it. A
stock Void box syncs nothing on its own — there is no `xbps` cron job and the
COSMIC desktop ships no update indicator — so an install could sit on hundreds
of outdated packages indefinitely without a single prompt. KDE Discover and
GNOME Software gained exactly this notifier on 2026-11-06
(`DiscoverNotifier`, `gs-plugin-xbps`'s startup check); the COSMIC store was
the odd one out.

### The fix

`cosmic-store` now has a second, windowless mode. `--notifier` branches out of
`main()` right after `Cli::parse()`, **before** the single-instance check, so a
poller can never swallow a real launch, and runs `crate::notifier::run()` in a
plain blocking loop — no async runtime, no toolkit, no `WAYLAND_DISPLAY`
required.

`/etc/xdg/autostart/cosmic-store-notifier.desktop` (`Exec=cosmic-store
--notifier`, `NoDisplay=true`) starts it with the session.

What the loop does:

- ticks every 60 s, looking at the clock and at the repodata files
  (`/var/db/xbps/*/x86_64-repodata`, compared by path + mtime + length);
- probes with `xbps-install -nSu` **unprivileged** — every 60 min
  (`update_check_interval_minutes`, floored at 5; `0` disables update checking
  and the notifier exits), and additionally when a repodata file changes,
  debounced to at most once every 5 min;
- parses the plain `<pkgver> update <arch> <repo> <size> <dsize>` lines itself
  (`field[1] == "update"`) instead of reusing the backend's `updates()`, which
  needs a locale and an AppStream cache and silently drops packages with no
  `AppInfo`;
- announces only when the set is non-empty **and** it differs from the last
  announced set (FNV-1a fingerprint) **or** 24 h have passed since the last
  announcement, so a persistent update nags once a day rather than every hour;
- writes `<state>/cosmic-store/update-notifier.state` (`fingerprint=`,
  `notified_at=`) only after a *successful* desktop notification, and clears
  it to `0`/`0` when a probe successfully reports nothing — so a set that gets
  installed elsewhere is not re-announced.

Text is localised through the store's own catalog: `update-notifier-summary`
(a plural on `$count`), `update-notifier-more` for the overflow beyond three
package names, and `update-notifier-action = Review updates`.

Clicking the notification runs `cosmic-store --updates`, which is the second
new flag: `App::init` selects `NavPage::Updates` as the start page instead of
the default Browse page. No explicit fetch is needed — `Message::BackendUpdate`
already calls `update_backend_updates()` for every backend, and that path goes
through `load_caches(refresh = true)`, so opening from the notification also
syncs the index revision 11 introduced.

### Hazards handled

- **The snapper shadow.** On an installer-provisioned box `/usr/bin/xbps-install`
  and `/usr/sbin/xbps-install` are both symlinks to
  `/usr/local/sbin/void-xbps-snap`. The notifier prefers
  `/usr/bin/xbps-install.real` when it exists and only falls back to
  `/usr/bin/xbps-install`, so a background poll can never take a snapper
  snapshot pair. It runs unprivileged, so pkexec's `exec.path` annotations are
  not in play either.
- **Being killed.** The probe runs under `timeout 300`; a non-zero exit or a
  timeout is a probe failure that leaves the state file untouched and retries
  on the next interval rather than every tick.
- **Notification threading.** `notify_rust::Notification::show()` runs on a
  spawned thread that then blocks in `wait_for_action(Timeout::Never)`; only
  the `show()` result is reported back over an `mpsc` channel, so
  `NotificationHandle` never has to be `Send` and state is written after the
  banner actually appeared. `wait_for_action` also fires on `"__closed"`, so
  only `action == "default"` spawns the store.
- **Config drift.** The interval is re-read from `cosmic-store`'s config on
  every tick, so changing it (or setting `0`) takes effect without a restart;
  an unreadable config falls back to 60.

### Build and verification

- `revision=11 → 12`; `cosmic-store-1.11.0_12` is 15,240,217 B, sha256
  `6bc565f0dbf706d87d0cdb0e92b846a7c4e3565c9565673ca6bf490b0caf3fec`; the
  packaged binary is 47,293,536 B, sha256 `3fed574b…` (revision 11's was
  `b20d3583…`), and the package also carries
  `etc/xdg/autostart/cosmic-store-notifier.desktop`.
- Windowless mode proven: `env -u WAYLAND_DISPLAY -u DISPLAY timeout 10
  cosmic-store --notifier` runs to the timeout (exit 124) rather than dying for
  want of a display; `cosmic-store --updates` in the same environment dies at
  the compositor, which is correct for a GUI mode.
- Single-instance bypass proven: with a GUI store running, a second
  `cosmic-store --notifier` stayed alive, and `cosmic-store --updates` exited 0
  after forwarding to the existing window.
- Parse + notify + state proven with a temporary `xbps-install.real` shim:
  four synthetic `update` lines produced
  `fingerprint=17233252979556304251` (independently recomputed from the same
  sorted pkgvers) and a `notified_at` stamp — so the parse, the plural summary,
  the overflow line and a successful `show()` all ran; re-running with the same
  set left the stamp alone; an empty set cleared state to `0`/`0`; a failing
  prober left it untouched and logged
  `update-notifier: update probe failed: xbps-install -nSu failed (exit 1)`.
  The shim was removed and the real `/usr/bin/xbps-install.real` restored
  (sha256 `1a5a8f6d…`).

### Known limits

- The notifier reads the **cached** repodata, like the sibling stores' notifiers.
  It cannot sync the index itself — that needs root — so it can lag behind the
  mirrors until something refreshes: the store's own check (which does sync
  since revision 11) or `xbps-install -S`. The notification therefore means
  "the index you have says N updates are waiting"; opening the store from it
  gets the current one.
- No notification when the notifier is not running: there is no system service,
  by design (matching Discover and GNOME Software). It starts with the desktop
  session and stops with it.
- Clicking the notification while a store window is already open only focuses
  that window; libcosmic forwards `subcommand_opt` but not the start page, so
  it will not jump to Updates.
- A GUI-initiated system update is still not cancellable (the child runs as
  root) — unchanged by this release.

## epoch-1.11.0-r7 — 2026-11-02

Store release: the store can finally see updates. Only `cosmic-store` is
re-cut.

### The defect

cosmic-store builds its update list from `xbps-install -nSu`, which answers
from the **cached** repodata and never syncs it. Upstream asks the backends for
a refresh — `update.rs` and `main.rs` call `backends(&locale, refresh = true)`
on "check for updates", on the periodic check, and after a repository change —
and `backend/mod.rs` threads the flag into `load_caches(refresh)`. This
backend's implementation was `fn load_caches(&mut self, _refresh: bool)` and
dropped it on the floor, and nothing else on a stock Void box syncs either
(there is no `xbps` cron job). A backend that ignores the flag therefore
reports "up to date" indefinitely while the mirrors move on.

That is what happened here: the cached index was two days old, 341 of its
package entries disagreed with the mirror's, and exactly three of those are
installed on this box — `snapper-0.13.1_2`, `libsnapper-0.13.1_2`,
`libboost_thread-1.91.0_1` — none of which the store ever offered. The
evidence is non-mutating (`xbps-install -un` against a freshly downloaded index
lists precisely those three) and is preserved with the release.

### The fix

`load_caches(refresh)` now calls the new function `refresh_index()` when
`refresh` is true, then builds the cache from whatever index is on disk.

`refresh_index()` runs the new package-owned helper
`/usr/libexec/cosmic-store-refresh` through `pkexec`. The helper is where the
two hazards live:

- **It must not snapshot.** On an installer-provisioned box `/usr/bin/xbps-install`
  and `/usr/sbin/xbps-install` are both symlinks to `/usr/local/sbin/void-xbps-snap`,
  whose `_should_snap()` matches a bare `-S`. A sync through the wrapper would
  take a snapper pair on every update check. The helper resolves the
  unshadowed `/usr/bin/xbps-install.real` the installer leaves behind, refusing
  any candidate that canonicalises to the wrapper, and falls back to a PATH
  lookup for a stock Void box.
- **It must be cheap.** The store re-checks every
  `config.update_check_interval_minutes` (default 60, and the user can set it
  to 1), so the helper throttles itself for ten minutes through
  `/run/cosmic-store-refresh.stamp` — deliberately on `/run`, so a reboot
  clears it.

It runs `-S` and only `-S`: fetching the index must never begin an upgrade, and
`-Syu` is a transaction that belongs behind the existing `auth_admin` update
action. stdin is `/dev/null`, so a "import this repository key?" question (only
ever asked for a repository nobody has trusted yet) reads EOF and declines
rather than silently importing a key from a background check.

The refresh is **best-effort**. A refused authorisation, a missing helper or an
unreachable mirror logs a warning and keeps the existing repodata. Propagating
an error would abort `load_caches` and leave the store empty, which is a much
worse failure than a stale update list.

### Reporting: refreshed, skipped, or failed

The helper distinguishes three outcomes so the log never claims work it did not
do: **0** the repositories were synced, **75** (`EX_TEMPFAIL`) nothing was done
because a sync happened inside the throttle window, **anything else** the sync
failed and the cached index stands.

The first cut of this change returned 0 for a throttled skip as well, and the
live proof caught the consequence immediately: with the interval set to one
minute, the second periodic tick logged `xbps: repodata refreshed` twenty
milliseconds after the first, having fetched nothing. A skip now logs
`xbps: repodata refresh skipped, the index was synced recently`; only a real
sync reports `refreshed`, and a failure still carries the reason
(`authentication was cancelled`, `pkexec could not run the helper`, or the exit
status) plus the tail of the output.

Waiting on a network fetch inside `load_caches` is safe here because
`backend/mod.rs` runs it through `tokio::task::spawn_blocking`, on the blocking
pool and not the UI thread; the store shows its "updating backends" state while
it happens.

### A fourth polkit action, deliberately not `auth_admin`

`org.freedesktop.policykit.pkexec.run-cosmic-store-xbps-refresh` is
`allow_active=yes` (`allow_any` and `allow_inactive` are `no`). A password
prompt on an hourly background check would make the feature unusable, so this
one grant is silent — and it is safe only because of where it is annotated:

- it is annotated on `/usr/libexec/cosmic-store-refresh`, a package-owned
  helper that ignores its arguments and hardcodes `xbps-install -S`. It cannot
  install, remove or upgrade anything.
- annotating `argv1 = -S` on `xbps-install` itself was rejected as a hole:
  pkexec matches only the **first** argument, so
  `pkexec xbps-install.real -S -y` followed by any package list would have run
  a full transaction with no password at all.

The three transaction actions stay `auth_admin` and unchanged.

### Detail

- New file `srcpkgs/cosmic-store/files/cosmic-store-refresh` (sha256
  `64d55f65…`, 4524 B), installed `0755` to `/usr/libexec/` by `do_install()`.
- Policy file gains the fourth action (sha256 `060aa28b…`, was `68d0e38b…`).
  The FILE name still does not match the ACTION ids, which is why a
  filename-based grep for them finds nothing.
- `srcpkgs/cosmic-store/files/xbps.rs` (sha256 `7d2929de…`, installed as
  `src/backend/xbps.rs`): `XBPS_REFRESH`, `refresh_index()`, `load_caches`
  honouring its argument, the `75` skip branch, and the exit-status wording
  factored into `describe_status()` so both call sites name a signal instead of
  `exit None`. Spawned paths and command shapes for the three transactions are
  unchanged (`-y --`, `-y -R --`, `-Syu -y`).
- Template `revision=10 → 11`.
- Package `cosmic-store-1.11.0_11.x86_64.xbps`, 15,183,935 B, sha256
  `1fcac757…`; `/usr/bin/cosmic-store` sha256 `b20d3583…` (`_10` was
  `917e9d1a…`).

### Served repository (root)

- `cosmic-store-1.11.0_11` added; `_10` and every earlier build stay on disk
  (build numbers are not pruned — the index lists only the newest).
- `x86_64-repodata` regenerated and **re-signed** with the unchanged key
  (DER sha256 `83ffd154…`), so the installer's fingerprint pin still matches.

### Verified

- The helper directly, through `pkexec`, with no authentication prompt (it is
  `allow_active=yes` and this shell's session is active): five repositories
  synced, exit 0, stamp written; a second call inside the ten-minute window
  exits 75 in 19 ms and touches nothing.
- The store's own path, with `update_check_interval_minutes = 1` and
  `RUST_LOG=info`: the first periodic tick logged `xbps: repodata refreshed`
  after 5.6 s of work and replaced the cached index (new digest, new size);
  the following tick logged `xbps: repodata refresh skipped, the index was
  synced recently` twenty milliseconds later; a tick that landed mid-transaction
  logged `cannot check for updates, operations are in progress`.
- The hand-off in the real GUI: "Install all" prompted for authentication (the
  `update` action is `auth_admin`), ran `-Syu -y` through
  `/usr/local/sbin/void-xbps-snap`, and took snapper pair **#18 pre / #19 post**
  (`xbps system update`). `snapper` and `libsnapper` moved to `0.13.1_3`,
  `libboost_thread` to `1.92.0_1`, `xbps-install -nSu` is empty and the store's
  update list cleared. The refresh helper deliberately does not snapshot, so
  that pair is the only one in the window.
- Negative control: the previously installed `_10` binary contains none of
  `repodata refreshed`, `refresh skipped`, `/usr/libexec/cosmic-store-refresh`,
  while `_11` contains all three.

### Known limits

- Only `cosmic-store` honours the refresh flag after this change. KDE Discover
  and GNOME Software derive their update lists from `-nSu` the same way and
  still never sync, so the stale-index class of bug remains there. Both would
  need the same treatment (and, in Discover's case, a helper plus an action).
- The refresh is a metadata download only. It cannot repair a cached index that
  a failed fetch left truncated; `xbps-install.real -S` by hand is still the
  recovery for that.

## epoch-1.11.0-r6 — 2026-11-01

Store release: the dedicated polkit action is reachable again on every box this
project provisions. Only `cosmic-store` is re-cut — the other 31 packages are
byte-identical to `epoch-1.11.0-r4`.

The store ships three polkit actions
(`org.freedesktop.policykit.pkexec.run-cosmic-store-xbps-{install,remove,update}`,
all `auth_admin`), each annotated with the `exec.path` it is meant to match.
pkexec does not compare that annotation against the path the caller spawns: it
canonicalises the program with `realpath()` first and looks the action up by
the result. On an installer-provisioned Void box `/usr/bin/xbps-install` and
`/usr/sbin/xbps-install` are both symlinks to `/usr/local/sbin/void-xbps-snap`
(the snapper pre/post wrapper, `[PKG-SNAP-*]`), so the canonical path is the
wrapper and the `/usr/bin/xbps-install` annotation could never match. Every
install and every system update therefore fell through to pkexec's generic
`org.freedesktop.policykit.exec` action: the dialog showed pkexec's stock argv
wording instead of this store's message, and any per-action rules.d policy
keyed on these ids was inert.

The install and update actions now annotate
`/usr/local/sbin/void-xbps-snap`. `remove` keeps `/usr/bin/xbps-remove`: the
wrapper shadows `xbps-install` only, so that path is already canonical. Nothing
about what is authorised changes — all three actions stay `auth_admin`, the
wrapper passes argv through unchanged, and on a stock Void box (no shadow) the
canonical path of `xbps-install` is the real binary, nothing matches, and
lookup falls back to the same generic prompt as before.

### Detail

- Policy file `org.freedesktop.policykit.pkexec.cosmic-store.xbps.policy`
  sha256 `68d0e38b…` (was `39e2aaf2…`). The FILE name still does not match the
  ACTION ids, which is why a filename-based grep for them finds nothing.
- `src/backend/xbps.rs` gains a comment block explaining the canonicalisation
  trap, and its module docs now name the real action ids
  (`…pkexec.run-cosmic-store-xbps-*`) instead of a stale `…cosmic-store.xbps.*`
  form. The spawned paths are unchanged (`/usr/bin/xbps-install`,
  `/usr/bin/xbps-remove`) and so are the command shapes: `-y --` install,
  `-y -R --` remove, `-Syu -y` update.

### Served repository (root)

- `cosmic-store-1.11.0_10` added; `1.11.0_9` and every earlier build stay on
  disk (build numbers are not pruned — the index lists only the newest).
- `x86_64-repodata` regenerated and **re-signed** with the unchanged key
  (DER sha256 `83ffd154…`), so the installer's fingerprint pin still matches.

### Release history: r5 → r6

`epoch-1.11.0-r5` shipped the `_10` package but **not** a new index: the served
`x86_64-repodata` stayed byte-identical to r4's and kept advertising
`cosmic-store-1.11.0_9`, so a client installing from r5 still received rev 9.

`tools/publish.sh` built the index with `xbps-rindex -f -a` over a lexically
sorted file list, and that mode is last-wins per package name. A stage holding
`_1 _10 _2 _8 _9` therefore added `_10` first and `_9` last, and `_9` won.
Single-digit revisions had always happened to sort in build order, so the trap
first appeared at `_10`. The script now adds the **newest** build of each
package name (compared with `xbps-uhelper cmpver`) and prints a line whenever
that differs from the lexical choice — which is what flagged the r5 index
before r6 was cut.

`epoch-1.11.0-r6` is the corrected publish. r5 stays where it is: a published
tag is never moved.

### Verification (live, this box)

`cosmic-store-1.11.0_10` was installed **from the served repository** (not from
the build tree), so the served bytes have a client-side proof: RSA signature
verified, origin recorded as the store URL, `filename-sha256 43476bde…`, and
the installed policy `68d0e38b…`. `pkaction` reports all three actions
`auth_admin` with `exec.path` → `/usr/local/sbin/void-xbps-snap` on install and
update and `/usr/bin/xbps-remove` on remove — the load-bearing check for the
XML, since polkit rejects a malformed policy file outright and the actions then
disappear.

The fix was then driven through the store's own UI, because `pkaction` only
shows what the policy *says*; the prompt shows which action polkit *matched*.
Installing a package produced "Authentication is required to install software"
— this policy's `<message>` for `…run-cosmic-store-xbps-install`, where the
pre-revision-10 policy fell through to the generic
`org.freedesktop.policykit.exec` and read "…run
/usr/local/sbin/void-xbps-snap as the super user". Removing it produced
"…to remove software", i.e. `…run-cosmic-store-xbps-remove`, confirming the
deliberate half of the fix (`/usr/bin/xbps-remove` needs no wrapper path).

A 43-package transaction (VLC plus 42 dependencies) was also installed through
the UI: the progress bar stayed monotonic through the download leg, reached
100 %, and the transaction completed with no error sheet. That is the shape the
phase-weighted tracker was built for, and it behaves as documented — the
granularity limit is xbps buffering its `unpacking`/`configuring` lines, and
those arrive in a leg that is over in milliseconds. A flatpak install from the
same UI was clean, which is the control: that backend is upstream and untouched
here.

Uninstalling it through the same UI confirmed the recursive remove. The shipped
argv is `-y -R --`, and `-R` is load-bearing: a plain `xbps-remove` takes only
the named package and leaves the whole dependency set behind as orphans (an
earlier measurement on this box had one `vlc` removal leave 59 of them). After
the UI uninstall the package count returned to its exact pre-install figure
(799) and `xbps-remove -n -o` reported **no orphans at all**, so the closure is
cleaned rather than accumulated. A single-package removal (`cowsay`) could not
show this — it has no dependencies.

Install and remove take **no** snapper pair, by design: `void-xbps-snap`
snapshots on `-S`/`-S*`/`-u` only, and a store install/remove spawns a bare
`-y -- <pkg>`. Only the update action (`-Syu -y`) snapshots.

## epoch-1.11.0-r4 — 2026-10-30

Cosmetic store release: a helper killed by a signal now says so. Only
`cosmic-store` is re-cut — the other 31 packages are byte-identical to
`epoch-1.11.0-r3`.

`epoch-1.11.0-r3` left one rough edge in the failure message. `run_privileged()`
built it with `{code:?}` over `ExitStatus::code()`, and that call returns
`None` for a child that died from a **signal** rather than exiting — a
cancelled transaction, an OOM kill, a crash. The store therefore reported the
nonsense `exit None` exactly when it had the most to explain.

The message is now derived from the exit status: `exit N` for an ordinary
non-zero exit, `terminated by signal N` when the child was signalled, and
`terminated abnormally` for the neither case (`signal()` comes from
`std::os::unix::process::ExitStatusExt`).

Nothing else changed. pkexec's `126` (dialog dismissed) and `127` (helper not
runnable) keep their dedicated wording, the helper output is still collected
rather than forwarded, and the progress tracker is untouched.

### Detail

- Packaged binary `ca81ef9c…` (was `f566bd74…`); `strings` shows both new
  literals and no remaining `failed (exit ` interpolation, and the previous
  binary is the negative control for that check.
- This changes reporting only: a GUI operation still cannot be cancelled,
  because the child runs as root and cancelling needs a fourth privileged
  polkit action. What the release fixes is the honesty of the report after an
  external kill.

### Served repository (root)

- `cosmic-store-1.11.0_9` added; `1.11.0_8` and every earlier build stay on
  disk (build numbers are not pruned — the index lists only the newest).
- `x86_64-repodata` regenerated and **re-signed** with the unchanged key
  (DER sha256 `83ffd154…`), so the installer's fingerprint pin still matches.
  The index now carries 67 builds; every one of the 32 templates still
  resolves to its own `version_revision`.

## epoch-1.11.0-r3 — 2026-10-30

Store release: `cosmic-store`'s xbps backend update actually runs, and it
reports real progress while it does. Only `cosmic-store` is re-cut — the
other 31 packages are byte-identical to `epoch-1.11.0-r2`.

Two user-visible defects were reported from the GUI:

1. **The system update claimed success and changed nothing.** `run_privileged()`
   spawned `pkexec /usr/bin/xbps-install -Syu` with `stdin(Stdio::null())`.
   xbps asks "Do you want to continue?" unless `-y` is on the command line;
   it read EOF (interpreted as *no*), printed `Aborting!` and **still exited
   0**, so the store rendered a completed transaction over an update that
   never happened. `xbps-install -Syu -y` fixes it. `argv1` stays `-Syu` —
   the polkit policy annotates `exec.argv1="-Syu"` and pkexec matches on it,
   so a combined `-Suy` would fall through to the generic action.
2. **Installs and updates showed no progress.** The helper now drains the
   child's stdout/stderr on their own threads and feeds a phase-weighted
   tracker, so the transaction bar moves like the flatpak backend's does.

### Detail

- `run_privileged()` takes an `FnMut(f32)` progress sink and `pump()`es both
  streams; the output is still **collected, never forwarded** to the store's
  log — forwarding it made Discover-style front ends raise a spurious
  "there was an issue" sheet for a successful transaction.
- Progress is phase-weighted and monotonic: prepare `1`, download `1–40` by
  bytes (`humanize_to_bytes` on xbps' `NNKB`/`NNMB` sizes), verify `40`,
  unpack `40–85` by package count, configure `85–99`, success `100`; the
  store divides by 100, the same `0..1` scale flatpak uses.
- Pure removals print no `[*]` headers, so the `Phase::Prepare` arm reports
  `1 + 98 × removed/planned_removals`; both of xbps' success wordings
  (`installed ... successfully` and ``Removed `pkg' successfully``) are
  matched.
- Known granularity limit (xbps upstream, not this backend):
  `xbps-install.real` imports `fflush` but neither `setvbuf` nor
  `setlinebuf`, so only the download callback flushes per line
  (`fetch_cb.c`). Everything else is buffered 4 KiB when stdout is a pipe,
  measured as 52 consecutive `unpacking`/`configuring` lines arriving inside
  21 ms. The download leg is therefore smooth and unpack/configure step
  coarsely — the bar stays faithful, it just cannot show lines xbps has not
  flushed. A pty would smooth it, but that belongs in
  `/usr/local/sbin/void-xbps-snap` (pkexec matches the store's spawn line
  against `exec.path`), so it is documented rather than implemented.

### Verification (live, this box)

- A 13-update system update: bar to 100 %, `xbps-install -nSu` clean
  afterwards, snapper pre/post pair taken (`#49`/`#50`), and all four kernel
  `vmlinuz`/`initramfs` pairs on the ESP md5-identical to `/boot`.
- `libreoffice` (152 packages, ~483 MB): bar 1 → 2 → 5 → 7 → 8 → 18 → … → 36
  through the download leg, then 42 (seven packages unpacked), 88 (configure
  leg), 93, done. Final state 19 `libreoffice*` packages installed, nothing
  pending, no error in the store log.
- `vlc`: 1 → 2 → 40 → 100 %; `sl` install and remove through the shim
  harness (28 checks) still pass.
- Failure path unchanged in shape: a killed child surfaces
  `operation N failed:` plus the tail of the collected output.

### Served repository (root)

- `cosmic-store-1.11.0_8` added (`1.11.0_1` keeps serving as rollback, like
  every earlier build; the index lists only the newest).
- `x86_64-repodata` regenerated and **re-signed** with the unchanged key
  (DER sha256 `83ffd154…`), so the installer's fingerprint pin still matches.
  The index now carries 66 builds; every one of the 32 templates still
  resolves to its own `version_revision`.
- The encrypted signing PEM cannot be read by `xbps-rindex` on a pipe: it
  must be decrypted first, and the decrypted copy is shredded immediately
  after signing.

## epoch-1.11.0-r2 — 2026-10-26

Parity release: the served index is brought back in line with the templates.

`epoch-1.11.0` shipped **30 of the 32** packages at `1.11.0`. The 2026-10-24
build round built the set through `./xbps-src pkg cosmic-desktop-full`, and
`cosmic-desktop-minimal` and `cosmic-store` are not in that metapackage's
dependency tree, so both were never rebuilt and the index kept serving their
`1.10.0` builds. Nothing was broken for users — both `1.10.0` builds are
internally consistent and every dependency they declare is satisfied at
`1.11.0` — but the served repository and `srcpkgs/` disagreed, which is a
release-hygiene defect: a client asking for a *name* was served an older
build than the templates describe.

### Served repository (root)

- `cosmic-desktop-minimal-1.11.0_1` (meta package) and
  `cosmic-store-1.11.0_1` (Rust build) added, built from the already-pinned
  `1.11.0` templates. The `1.10.0` builds of both stay on disk as rollback,
  like every earlier build; the index lists only the newest per package.
- `x86_64-repodata` regenerated and **re-signed** (`xbps-rindex -s`). Every
  repack must be re-signed: an unsigned index makes clients abort at the
  key check (2026-10-09 incident). The signing key is unchanged
  (DER sha256 `83ffd154…`), so the installer's fingerprint pin still matches.
- All 32 templates now resolve to their `1.11.0` build in the served index;
  `coordination/check-pins.sh` gains a **template-vs-index drift guard** so a
  future epoch bump cannot silently leave packages behind again.

### Packaging sources (`srcpkgs/`)

- No template changed in this release. `cosmic-store`'s carried patch
  `fix-system-appstream-refresh` is unchanged; it was re-verified against the
  `epoch-1.11.0` tree (upstream still hands the suffixed `"<name> (system)"`
  cache source id to `update_appstream_sync()`), so it is still required.

## epoch-1.11.0 — 2026-10-24

Tracking upstream COSMIC **`epoch-1.11.0`**.

### Packaging sources (`srcpkgs/`)

- All 32 templates repinned to the upstream `epoch-1.11.0` tag by
  `tools/sync-upstream.sh --apply`. `libcosmic` stays commit-pinned (its
  tagged releases lag the epoch; build style `cargo-gitmodules`),
  `pop-launcher` keeps its own version `1.2.7`, and the two meta packages
  track the epoch. `PINS.md` was regenerated with the new checksums.
- The package set is unchanged from `epoch-1.10.0` — 29 `cosmic-*` packages
  plus `libcosmic`, `pop-launcher` and `xdg-desktop-portal-cosmic`. Nothing
  added, nothing dropped.
- All 14 carried patches were re-checked with `git apply --check -p1` against
  the extracted `epoch-1.11.0` tarballs and still apply cleanly; none could
  be dropped on this bump.

### Served repository (root)

- `1.11.0` builds of the component set, signed and indexed, published under
  the `epoch-1.11.0` tag.
- Two rounds of follow-up commits repaired the publish: the first publish left
  **23 packages unsigned** (`396aac3`), `cosmic-icons` had no `.sig2` at all
  (`da06595`), `pop-launcher`'s signature did not verify (`33ec9b0`), and
  `cosmic-greeter-1.11.0_1` was only added afterwards (`65169e3`). The
  `epoch-1.11.0` tag covers the first publish only, so `epoch-1.11.0-r2` is
  the first tag whose tree matches what the index serves.

### Known gaps at this tag, fixed in `epoch-1.11.0-r2`

- `cosmic-desktop-minimal` and `cosmic-store` were never rebuilt for this
  epoch (outside `cosmic-desktop-full`'s dependency tree), so the index kept
  their `1.10.0` builds.

## epoch-1.10.0 — 2026-10-11

Initial release, tracking upstream COSMIC **`epoch-1.10.0`**.

### Packaging sources (`srcpkgs/`)

- 32 templates for the full COSMIC set — 29 `cosmic-*` packages plus
  `libcosmic`, `pop-launcher` and `xdg-desktop-portal-cosmic` — each pinned to
  the upstream `epoch-1.10.0` tag, with the canonical maintainer
  `Void Cosmic <noreply@github.com>` (`tools/check-maintainer.sh` gates it).
- `tools/sync-upstream.sh` tracks the aggregate `epoch-*` tags: `--check`
  reports drift, `--apply` repins `version=` / `distfiles=` / `checksum=` and
  regenerates `PINS.md`. Each template also ships a Void `update` file.
- Special cases: `libcosmic` is commit-pinned (its tagged releases lag the
  epoch) with build style `cargo-gitmodules`; `pop-launcher` keeps its own
  version `1.2.7`; `cosmic-desktop-full` / `cosmic-desktop-minimal` are meta
  packages whose version tracks the epoch.
- `tools/build-and-stage.sh` overlays `srcpkgs/` and `common/` onto a
  `void-packages` checkout to build the set.

### Served repository (root)

- The built and signed binary repository: 34 `.xbps` packages + 34 `.sig2`
  signatures and the signed `x86_64-repodata` index, served over GitHub Pages
  at <https://bluesloth.github.io/void-linux-cosmic/>. The signing key
  (DER sha256 `83ffd154…`) is reused, so the installer's fingerprint pin is
  unchanged.
- Within epoch `1.10.0`, `cosmic-settings` and `cosmic-store` each carry a
  `_2` revision alongside `_1`.

### Carried patches

- `cosmic-store`: patch the system-remote AppStream refresh (revision 2).
  `src/backend/flatpak.rs` passed the `"<name> (system)"` cache source id into
  `update_appstream_sync()`, which expects a plain remote name, so system
  remote AppStream could never be fetched on a cold start
  (`Remote "flathub (system)" not found`). Upstream tracking: pop-os issues
  #216 / #514 (and #551, same root cause). Re-verify against the next epoch.

### Licensing

- `LICENSING.md` (per-package licence notice). The templates configure builds
  of upstream COSMIC releases and each declares its own `license=` field; the
  upstream projects are System76 / Pop!_OS (`pop-os/*`).
