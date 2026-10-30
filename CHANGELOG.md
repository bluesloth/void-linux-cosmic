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

## Unreleased

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
