# Licensing

`void-linux-cosmic` is a binary **xbps package repository** (`.xbps` packages +
the signed `x86_64-repodata` index) whose packaging templates live in
`srcpkgs/`.

The repository itself carries no single license; each package's license is
declared in its `srcpkgs/<pkg>/template` `license=` field — mostly
`GPL-3.0-only`, with `MPL-2.0`, `CC-BY-SA-4.0`, and `GPL-3.0-or-later`.

The templates are built from scratch, tracking upstream System76 / Pop!_OS
COSMIC sources.
