# Void_Linux_Cosmic

Packaging sources for the [COSMIC](https://github.com/pop-os/cosmic-epoch)
desktop on Void Linux: the `xbps-src` templates for the whole COSMIC set, plus
tooling to track upstream and to build.

This repository is both the **source** and the **published** home. It holds
the `xbps-src` templates above, and at its root the built `.xbps` packages and
the signed `<arch>-repodata` index that the installer consumes, served over
GitHub Pages at <https://bluesloth.github.io/void-linux-cosmic/>.

## Layout

```
srcpkgs/            xbps-src templates (one directory per package)
  cosmic-*/         the COSMIC components, in dependency order
  libcosmic/        the COSMIC GUI library
  pop-launcher/     the launcher service
  xdg-desktop-portal-cosmic/
common/
  build-style/cargo-gitmodules.sh   cargo build style with git-submodule support
tools/
  sync-upstream.sh   track the upstream `epoch-X.Y.Z` releases
  check-maintainer.sh  fail if a non-canonical maintainer identity is present
  build-and-stage.sh   overlay onto a void-packages checkout and build
  publish.sh           sign, stage, commit, tag and push a release
docs/UPSTREAM.md    the tracking model and release procedure
PINS.md             the tracked epoch and per-template checksums
CHANGELOG.md        release history (tagged `epoch-<X.Y.Z>`)
*.xbps, *.sig2,
<arch>-repodata     the built binary repo, served from the repository root
```

## Tracking upstream

COSMIC ships as `epoch` releases: the aggregate repository
[`pop-os/cosmic-epoch`](https://github.com/pop-os/cosmic-epoch) and every
component repository carry the same `epoch-X.Y.Z` tag. Each template fetches

```
https://github.com/pop-os/<component>/archive/refs/tags/epoch-<version>.tar.gz
```

so a single tag drives the whole set.

```sh
tools/sync-upstream.sh            # report drift; exit 2 if a newer epoch exists
tools/sync-upstream.sh --list     # pinned vs upstream
tools/sync-upstream.sh --apply    # repin version=, distfiles= and checksum=
tools/sync-upstream.sh --pins     # regenerate PINS.md
```

See `docs/UPSTREAM.md` for the special cases (commit-pinned `libcosmic`,
`pop-launcher`'s own version, the meta packages).

## Building

```sh
XBPS_DISTDIR=/path/to/void-packages tools/build-and-stage.sh
```

This overlays `srcpkgs/` and `common/` onto a `void-packages` checkout and runs
`xbps-src pkg` for `cosmic-desktop-full` (which pulls the rest in). Building
COSMIC is heavy (Rust) - use a Void host or a build container.

## Publishing

The served repository is published with `tools/publish.sh`, which signs the
packages and the index (`xbps-rindex`), copies the `.xbps` / `.sig2` files and
the `<arch>-repodata` index into this repository's root, commits, tags the
release, and (with `--push`) pushes; GitHub Pages then serves them to the
installer.

```sh
tools/publish.sh --from DIR --privkey KEY --push     # stage a pre-built repo
XBPS_DISTDIR=/path/to/void-packages tools/publish.sh --build --privkey KEY --push
```

Releases are tagged from the tracked epoch: the first publish of an epoch is
`epoch-<X.Y.Z>` (see `PINS.md`); a republish within the same epoch takes
`-r2`, `-r3`, …. `tools/publish.sh` computes the next tag from the tags already
on the remote, so re-runs are idempotent. Run it without `--push` to commit and
tag locally first, and with `--dry-run` to preview. See `CHANGELOG.md` for the
release history.

## Identity

Every template declares the same canonical maintainer:

```
maintainer="Void Cosmic <noreply@github.com>"
```

`tools/check-maintainer.sh` enforces this across the tree.

## Licensing

The packaging templates configure builds of upstream COSMIC releases; each
template declares its own `license=` field. See the upstream projects for the
source terms.
