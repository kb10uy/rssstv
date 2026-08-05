# Continuous Integration and Releases

Two workflows in `.github/workflows/` cover the repository: `ci.yml` checks
every change, and `release.yml` builds and publishes what a tag names.

## CI

`ci.yml` runs on pushes to `master` and on pull requests, in two jobs.

`check` runs the commands `AGENTS.md` requires before a change is complete:
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets` with
warnings denied, `cargo test --workspace`, `cargo build --workspace`, and
`cargo build -p rssstv-sstv --no-default-features`.

It runs on Linux alone. That is a deliberate asymmetry rather than full
coverage: Linux selects `platform/other.rs` and the in-window menu bar, which
is the configuration least likely to be exercised during development on
Windows, so CI covers the side the author does not. macOS compiles nowhere
until a release builds it, and `platform/macos.rs` is the code this leaves
unchecked.

Only ALSA needs a system package to build. The window, the graphics context,
and the Wayland and X11 clients are all opened at run time rather than linked,
so `libasound2-dev` is the whole list.

`licenses` checks the dependency graph rather than the code: `cargo deny check
licenses` against `deny.toml`, `cargo deny check advisories` against the
RustSec database, and `cargo about generate` against `about.toml` and
`about.hbs`. The last one is there because the release archives carry that
page, and a template that cannot produce it should fail on the change that
broke it rather than during a release.

An advisory published against a dependency turns CI red on changes that have
nothing to do with it. That is the intended behavior: the alternative is
learning about it when a release is already being cut.

## Releases

`release.yml` runs on a pushed tag matching `v*`, and can be dispatched
manually with the tag to build. Cutting a release is:

```text
git tag v0.1.0
git push origin v0.1.0
```

`prepare` resolves the tag and refuses to continue unless it matches the
workspace version, because the executables carry the version compiled into
them and Windows records it in the resource. Bump `version` in the workspace
`Cargo.toml` before tagging.

`build` is a matrix of three targets, each on its own runner:

| Target | Runner | Archive |
| --- | --- | --- |
| `x86_64-pc-windows-msvc` | `windows-latest` | `.zip` |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `.tar.gz` |
| `aarch64-apple-darwin` | `macos-latest` | `.tar.gz` |

Each builds `rssstv`, `encode-wav`, and `decode-wav` with `--locked`, so a
release is built from the committed `Cargo.lock` and not from whatever resolves
that day.

Each archive holds the three executables, `LICENSE`, `README.md`, `docs/`,
`templates/`, and a `licenses.html` generated on that platform. The page is
generated per platform rather than once for all three because the dependency
graph differs by target: a page built on Linux would list neither `muda` nor
`windows-sys`. The Linux archive also carries `assets/rssstv.desktop` and
`assets/icon.png`, which a Wayland compositor needs to find the window icon.

## The Windows C runtime

`.cargo/config.toml` links the MSVC CRT statically for
`x86_64-pc-windows-msvc`. A dynamically linked build imports
`VCRUNTIME140.dll`, which is part of the Visual C++ redistributable and not of
Windows, while the `api-ms-win-crt-*` imports beside it resolve to
`ucrtbase.dll` and are an operating system component. Only the first one is a
problem, and it is a problem this distribution cannot solve any other way: the
archive has no step that could install a redistributable, and a missing DLL
fails in the loader before the program can report anything.

Statically linking takes the UCRT along with it, so a CRT fix now arrives by
rebuilding rather than through Windows Update. That is the accepted cost. The
alternative of shipping `VCRUNTIME140.dll` beside the executables has the same
servicing property, since an application-local copy is not updated either, and
adds a way to break: an executable copied out of the extracted directory stops
starting.

Statically linked executables import operating system libraries alone, which is
worth checking after a dependency that brings C code is added. The archived
`encode-wav.exe` imports four.

`release` collects the archives, writes `SHA256SUMS` over them, and publishes
a GitHub release. Re-running a tag that was already released replaces its
archives rather than failing, so a rebuild is a re-run.

Nothing is code signed. A macOS user has to clear the quarantine attribute
before the first run, and the release notes say so.
