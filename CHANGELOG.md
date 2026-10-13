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
