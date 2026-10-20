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

## Installing (end user, on an existing Void install)

The published repository is served from this repository's root over GitHub
Pages. To consume it on a running Void system, as root:

```sh
# 1. add the repository (the 59- prefix sorts after Void's stock repo configs).
#    Copy-paste as-is; drop the "sudo" if you are already root. The tee
#    command prints the line back so you can see what was written:
echo 'repository=https://bluesloth.github.io/void-linux-cosmic/' | sudo tee /etc/xbps.d/59-cosmic.conf

#    ...or as a heredoc (same file, same content):
sudo tee /etc/xbps.d/59-cosmic.conf >/dev/null <<'EOF'
repository=https://bluesloth.github.io/void-linux-cosmic/
EOF

# 2. refresh the indexes; xbps will ask to import the repository signing
#    key — answer y
xbps-install -S

# 3. install whichever package you want
xbps-install cosmic-desktop-full      # the whole desktop, ready to use
xbps-install cosmic-desktop-minimal   # the session itself, no extras
xbps-install cosmic-store             # just the software store app
xbps-install cosmic-greeter           # just the login screen (not in either meta-package)
```

Step 3 is ordinary xbps: dependencies are resolved and pulled automatically,
and any of the packages above (or any single `cosmic-*` component) can be
installed on its own.

Before trusting anything from this repository, verify that the key imported
in step 2 has the expected fingerprint:

```
SHA256 83ffd154d1dee504ea384a0c31b12e072d5884f7f75eb950e30ace852d7d2e7b
```

xbps has no command to print a repository key's fingerprint, so compute it
from the stored key directly (as root):

```sh
for k in /var/db/xbps/keys/*.plist; do
    b=$(sed -n '/<key>public-key<\/key>/,/<\/data>/p' "$k" | tail -n +2 \
        | sed -e 's/.*<data>//' -e 's|</data>||' -e 's/[[:space:]]//g')
    fp=$(printf '%s' "$b" | base64 -d 2>/dev/null \
         | openssl pkey -pubin -outform DER 2>/dev/null \
         | openssl dgst -sha256 2>/dev/null | sed 's/.*= *//')
    printf '%s  %s\n' "${fp:-UNREADABLE}" "${k##*/}"
done
```

The pinned fingerprint above must appear in the output. The other entries
will be your other trusted repositories (Void's stock keys look like
`05473705…` and `f0da25a2…`); if the pinned one is missing, or an
unrecognised key is trusted for this repository, do not install — investigate
first.

Installed keys live in `/var/db/xbps/keys/*.plist`. This is a third-party
repository: the signature chain above, not Void's official infrastructure, is
what you are trusting. To verify what is being served, audit this repository
on GitHub and compare against `x86_64-repodata`.

### `cosmic-desktop-minimal` vs `cosmic-desktop-full`

Both are meta-packages, and both already contain **every COSMIC component**:
the compositor (`cosmic-comp`), panel, settings, the session
(`cosmic-session`), and the full app suite (editor, files, terminal, player,
viewer, screenshot tool, and so on). They differ only in the supporting stack
pulled in around the session:

| Component                                          | minimal | full |
| -------------------------------------------------- | ------- | ---- |
| COSMIC session + all COSMIC apps                   | yes     | yes  |
| Audio stack (PipeWire, wireplumber, GStreamer, Bluetooth audio) | no | yes |
| XWayland (running X11 apps)                        | no      | yes  |
| Bluetooth (`bluez`)                                | no      | yes  |
| Fonts (DejaVu, Liberation, Noto, Noto emoji)       | no      | yes  |
| `pop-launcher`, `xdg-desktop-portal`, `xdg-user-dirs` | no   | yes  |
| Base runtime deps (dbus, elogind, mesa, wayland, fontconfig) | yes | yes |

- Choose **minimal** if you already run your own audio/fonts/portal stack, or
  want to assemble exactly the extras you need on top.
- Choose **full** for a ready-to-use desktop out of the box.

Note that `cosmic-greeter` (the COSMIC login screen) is packaged here but is
**not** pulled in by either meta-package — install it separately if you want
it (`xbps-install cosmic-greeter`); otherwise log in through the display
manager you already have and select the COSMIC session.

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
