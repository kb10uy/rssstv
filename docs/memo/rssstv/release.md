# Continuous Integration and Releases

Three workflows in `.github/workflows/` cover the repository: `ci.yml` checks
every change, `release.yml` builds and publishes what a tag names, and
`deploy.yml` publishes the browser demo.

## CI

`ci.yml` runs on pushes to `master` and on pull requests, in four jobs.

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

`manual` renders `docs/help/` with pandoc, for the same reason: the archives
carry the manual, so a source or template that cannot be rendered should fail
on the change that broke it.

`wasm` builds `web-demo` for `wasm32-unknown-unknown` and runs Clippy against
that target, then builds the page with `wasm-pack`. It is separate from `check`
for the reason the no-std jobs are: the host build compiles the JavaScript
bindings to stubs nothing calls, so it proves nothing about the target the demo
ships to. Building the page rather than only the crate is what keeps a broken
deploy from reaching `master`.

## Deploying the demo

`deploy.yml` runs on pushes to `master`, builds the WebAssembly module into
`web-demo/www`, and uploads that directory to Cloudflare Workers with
`wrangler`. It needs two repository secrets, `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`; the token needs permission to edit Workers scripts.

The build happens here rather than on Cloudflare because Cloudflare's build
image carries Node, Python, Go, and Ruby and no Rust. Connecting the repository
to Workers Builds would mean installing a toolchain on every deploy before
anything of this project is compiled, which is a worse trade than uploading a
directory that has already been built.

The demo is an assets-only Worker: `web-demo/wrangler.toml` names a directory
and no `main`, because the page decodes in the browser and there is nothing for
a Worker script to do. It is still a Worker rather than a Pages project, which
leaves room to put a script beside the assets later without moving the site.

Every path in the page is relative, which is what lets the same tree be served
from a local directory and from a Worker subdomain without rewriting.

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

It also renders the manual and uploads it as an artifact the three build jobs
take. The manual is the same text on all three platforms, unlike the license
page below, so building it once is both cheaper and the only way the three
archives are guaranteed to carry the same pages; the alternative is installing
pandoc on a Windows and a macOS runner to produce a copy that should be
identical anyway.

`build` is a matrix of three targets, each on its own runner:

| Target | Runner | Archive |
| --- | --- | --- |
| `x86_64-pc-windows-msvc` | `windows-latest` | `.zip` |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `.tar.gz` |
| `aarch64-apple-darwin` | `macos-latest` | `.tar.gz` |

Each builds `rssstv`, `encode-wav`, and `decode-wav` with `--locked`, so a
release is built from the committed `Cargo.lock` and not from whatever resolves
that day.

Each archive holds the three executables, `LICENSE`, `README.md`, the manual as
`help/`, `templates/`, and a `licenses.html` generated on that platform. The
development documentation under `docs/memo/` is not archived: it answers to this
repository's code rather than to the operator, and a release that carried it
would be handing out notes on an implementation instead of a manual. The page is
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
