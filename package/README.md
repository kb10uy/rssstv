# Packages

Packages for the `rssstv` desktop application. The two Linux packages
install:

- `/usr/bin/rssstv`
- `/usr/share/applications/rssstv.desktop`
- `/usr/share/icons/hicolor/512x512/apps/rssstv.png`
- `/usr/share/doc/rssstv/help/` — the operator's manual, which the Help menu
  falls back to when no `help/` directory sits beside the executable

Not packaged: the `encode-wav` and `decode-wav` command-line tools, and the
MMSSTV templates in [templates/](../templates) — copy those into
`~/.local/share/rssstv/templates` yourself. Installing them under
`/usr/share/rssstv/templates` is a possible follow-up.

## Arch Linux

`rssstv-bin` repackages the released x86_64 archive from GitHub Releases —
nothing is compiled. Bumping it to a new release means updating `pkgver` and
`sha256sums` (the hash is in the release's `SHA256SUMS`). Build and install:

```bash
cd package/arch && makepkg -si
```

The dependency license page ships in the archive and is installed as
`/usr/share/doc/rssstv/licenses.html`.

## Debian / Ubuntu

Built from the checked-out working tree — whatever is in it, not a released
tag. Prerequisites: rustc ≥ 1.85 (edition 2024), `build-essential`,
`libasound2-dev`, `mold` (named by `.cargo/config.toml` for every Linux build
of this tree), `pandoc`, and
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

## macOS

The release workflow stages `RSSSTV.app` — icon, `Info.plist` with the
microphone permission text, the manual under `Contents/Resources/help` — and
wraps it in a drag-and-drop disk image with `build-app.sh`. It runs on a Mac
too, given a built binary, a rendered manual, and a dependency license page:

```bash
bash package/build-app.sh target/release/rssstv target/help licenses.html RSSSTV.dmg
```

The bundle is ad-hoc signed, not notarized, so a downloaded image is
quarantined: open the first launch from the context menu, or
`xattr -dr com.apple.quarantine /Applications/RSSSTV.app`.
