# Upstream tracking

## The epoch model

COSMIC is released as **epochs**. The aggregate repository
[`pop-os/cosmic-epoch`](https://github.com/pop-os/cosmic-epoch) is a
submodule umbrella, and every component repository carries the **same**
`epoch-X.Y.Z` tag. Our templates therefore key the whole set off one number:

```
version=1.10.0
distfiles="https://github.com/pop-os/cosmic-session/archive/refs/tags/epoch-${version}.tar.gz"
```

Upstream tags we currently follow: `epoch-1.10.0` (latest stable).

## What `sync-upstream.sh` does

`tools/sync-upstream.sh` reads the `epoch-*` tags from `pop-os/cosmic-epoch`
(its single source of truth) and compares them with the pinned `version=` in
`srcpkgs/*/template`.

- **`--check`** (default) prints any template that is behind and exits `2`.
  Wire this into CI / the coordination drift check.
- **`--apply`** repins each epoch-tracking template:
  - `version=` is set to the new epoch *only* for packages that followed the
    previous epoch;
  - any literal `epoch-X.Y.Z` token in `distfiles=` is rewritten;
  - `checksum=` is recomputed from the downloaded tarball (cached in
    `distfiles/`, which is git-ignored);
  - `revision=` is reset to `1` when the template changes.
- **`--pins`** regenerates `PINS.md` from the current templates (no network).

Each template also ships a Void `update` file, so the standard
`xbps-src update-check` workflow works alongside this script.

## Special cases

**`libcosmic` is commit-pinned.** Its `distfiles` is a commit archive
(`archive/<sha>.tar.gz`), not an epoch tag, because libcosmic's tagged
releases lag the epoch that ships it. `sync-upstream.sh` reports it as
`commit-pinned (manual)` and never rewrites it. To advance it with an epoch
bump, read the submodule pointer the aggregate tag records:

```sh
# in a checkout of pop-os/cosmic-epoch:
git ls-tree epoch-1.11.0 libcosmic      # -> 160000 commit <sha>  libcosmic
```

then update `distfiles=` to
`https://github.com/pop-os/libcosmic/archive/<sha>.tar.gz` and refresh
`checksum=` (the tarball is cached by `--apply` if the URL is already updated,
or fetch it yourself and run `sha256sum`).

`libcosmic` also uses `build_style=cargo-gitmodules`, which is why the
repository carries `common/build-style/cargo-gitmodules.sh`.

## `pop-launcher` keeps its own version

`pop-launcher` has `version=1.2.7` (its own upstream version, from
`pop-os/launcher`) but is built from the COSMIC epoch tarball. `sync-upstream.sh`
repins the `epoch-X.Y.Z` token in its `distfiles=` but does **not** touch
`version=`.

## Meta packages

`cosmic-desktop-full` and `cosmic-desktop-minimal` have no `distfiles=`; their
`version=` simply tracks the epoch. Builds of either pull the rest of the set in
as dependencies.

## Carried patches

A few templates carry local patches under `srcpkgs/<pkg>/patches/`. Each
patch file documents its rationale and whether it is confirmed against the
current epoch. When bumping to a new epoch:

1. a patch that no longer applies (or whose rationale is gone) usually means
   upstream shipped the fix - verify, then drop it;
2. otherwise re-validate it against the new tree and keep it.

Carried patches today:

| package | patch | upstream status when added |
| --- | --- | --- |
| cosmic-store | `fix-system-appstream-refresh.patch` | pop-os/cosmic-store `epoch-1.10.0` still passes the `"<name> (system)"` cache source id as a flatpak remote name, so system-remote AppStream can never be fetched on a cold start (`Remote "flathub (system)" not found`). Symptoms reported upstream in issues #216 and #514; #551 (Fedora system remotes not shown) is open and likely the same root cause. |

## Release procedure

```sh
tools/check-maintainer.sh              # identity gate
tools/sync-upstream.sh                 # drift check (exit 2 when behind)
tools/sync-upstream.sh --apply         # repin + refresh checksums + PINS.md
# re-verify every patch in the "Carried patches" table above, then rebuild
git diff --stat                        # review
XBPS_DISTDIR=/path/to/void-packages tools/build-and-stage.sh
tools/publish.sh --from DIR --privkey KEY          # sign + commit + tag locally
tools/publish.sh --from DIR --privkey KEY --push   # ...and push
```

`tools/publish.sh` signs the packages and the index with `xbps-rindex`, copies
the `.xbps` / `.sig2` files and the `<arch>-repodata` index into the repository
root, commits them, and tags the release from the tracked epoch
(`epoch-<X.Y.Z>`, or `-r2`, `-r3`, … within the same epoch). Update
`CHANGELOG.md` as part of the release. Run without `--push` to commit and tag
locally, or with `--dry-run` to preview.

## Where the pieces live

- **this repository** - templates + tracking + build method, and the served
  binary repository at its root (built `.xbps` packages, their `.sig2`
  signatures and the `<arch>-repodata` index the installer consumes), published
  via GitHub Pages at <https://bluesloth.github.io/void-linux-cosmic/>;
- **installer / live** - the consumers.
