# Linux packages

Builds distribution packages for the `rssstv` desktop application from the
checked-out working tree — whatever is in it, not a released tag. Both
packages install:

- `/usr/bin/rssstv`
- `/usr/share/applications/rssstv.desktop`
- `/usr/share/icons/hicolor/512x512/apps/rssstv.png`
- `/usr/share/doc/rssstv/help/` — the operator's manual, which the Help menu
  falls back to when no `help/` directory sits beside the executable

Not packaged: the `encode-wav` and `decode-wav` command-line tools, and the
MMSSTV templates in [templates/](../templates) — copy those into
`~/.local/share/rssstv/templates` yourself. Installing them under
`/usr/share/rssstv/templates` is a possible follow-up.

Both builds link with mold on purpose: `.cargo/config.toml` names it for every
Linux build of this tree, so it is a build dependency here too.

## Arch Linux

`makedepends` covers the toolchain (cargo, mold, pandoc). Build and install:

```bash
cd package/arch && makepkg -si
```

The PKGBUILD has no `source=()`; it compiles the repository it sits in, and
`pkgver()` re-reads the workspace version so the package always matches the
tree. `RUSTFLAGS` from makepkg.conf is unset during the build — it would
silently replace the mold flag from `.cargo/config.toml` — so distribution
default flags are not applied to this package.

## Debian / Ubuntu

Prerequisites: rustc ≥ 1.85 (edition 2024), `build-essential`,
`libasound2-dev`, `mold`, `pandoc`, and
[cargo-deb](https://github.com/kornelski/cargo-deb)
(`cargo install cargo-deb`). Then:

```bash
bash package/build-deb.sh
```

The script renders the manual into `target/help` first, because cargo-deb
collects it as an asset but runs no build steps of its own, then produces
`target/debian/rssstv_<version>-1_<arch>.deb`. Install with apt so the
dependencies resolve:

```bash
sudo apt install ./target/debian/rssstv_*.deb
```
