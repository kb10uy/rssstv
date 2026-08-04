# Repository Guidelines

## Overview

This project implements SSTV (Slow Scan Television) software for amateur radio
in Rust. The goal is to port the core behavior of MMSSTV while separating its
signal-processing and protocol logic from the original Win32/VCL application.

The repository contains:

- `rssstv/`: the Rust application and workspace crate.
- `original/mmsstv/`: the original MMSSTV source code, included as a Git
  submodule and used as the behavioral reference.
- `docs/`: documentation, divided by subject. `docs/README.md` indexes it.
  - `docs/sstv/`: the protocols themselves — modes, timing, VIS, and FSKID —
    independent of any one implementation.
  - `docs/mmsstv/`: the behavior of the original application, including its DSP
    implementation and where it departs from published descriptions.
  - `docs/rssstv/`: this project — target architecture, the desktop
    application, and the transmit overlay format.

Put a new document under the directory matching what it is about. A protocol
description answers to the on-air signal, a description of MMSSTV answers to
its source, and a description of RSSSTV answers to this repository's code; a
document that would answer to two of those belongs in two documents.

Treat `original/mmsstv/` as reference material. Do not modify the submodule
unless the task explicitly requires changes to the original source.

## Architecture

The Rust implementation should keep the reusable SSTV core independent of UI,
audio backends, radio control, logging, and other platform integrations.

Use these conceptual boundaries as the implementation grows:

- `dsp`: FIR/IIR filters, FFT, Hilbert transforms, PLL, oscillators, and related
  numerical primitives.
- `sstv_modes`: mode definitions, image geometry, timing, VIS values, and shared
  protocol constants.
- `demodulator`: audio-to-frequency demodulation, sync detection, VIS/FSK mode
  detection, AFC, and receive state.
- `rx_decoder`: raster synchronization and conversion of demodulated samples to
  image pixels.
- `tx_encoder`: conversion of images to timed frequency sequences, including
  headers, VIS, scan lines, and identifiers.
- `modulator`: conversion of timed frequency values to PCM samples.
- `audio`: platform-specific audio input and output adapters.
- `image`: owned image buffers and RGB/luminance/chroma conversion.
- `application`: UI, history, templates, logging, PTT, CAT, and orchestration.

The intended data flow is:

```text
Receive:
AudioSource -> Preprocessor -> Demodulator -> RxDecoder -> ImageSink

Transmit:
ImageSource -> TxEncoder -> Modulator -> AudioSink
```

Keep core modules deterministic and testable with in-memory samples and images.
Platform modules should depend on the core; the core must not depend on platform
or application code. Prefer explicit ownership and bounded queues over global
state or implicitly shared buffers.

When reproducing MMSSTV behavior, consult both `sstv.cpp` and `Main.cpp`. The
original receive scan conversion, transmit line generation, VIS generation, and
queue scheduling are partly embedded in the VCL main form rather than isolated
in the original DSP classes.

## Code Style

- Use Rust edition 2024.
- Follow standard Rust naming and formatting conventions.
- Use LF line endings for all text files.
- Combine imports from the same crate into a single `use` statement within each
  module scope, except when different `cfg` attributes require separate imports.
- Avoid comments by default. Add comments only when explicitly requested by the
  user.
- Prefer the smallest correct implementation and avoid speculative abstractions.
- Model ownership and state transitions explicitly; avoid global mutable state.
- Keep platform-specific types and dependencies out of reusable core APIs.
- Use `rstest` features for parameterized tests, fixtures, and test cases where
  they improve coverage or reduce repetition.

## Documentation

- Write documentation in English.
- When a new implementation or fix changes behavior, APIs, architecture, mode
  support, limitations, or any other documented area, update the relevant
  documentation in the same change.

## Build and Test

This repository uses a Cargo workspace. Run commands from the workspace root.

- Build all workspace members with `cargo build --workspace`.
- Run all tests with `cargo test --workspace`.
- Run Clippy with `cargo clippy --workspace --all-targets`.
- Check formatting with `cargo fmt --all --check`.
- Apply formatting with `cargo fmt --all` when needed.

Before completing a code change, run at minimum:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build --workspace
```

Add focused unit tests for DSP and protocol behavior. Prefer deterministic test
vectors and parameterized `rstest` cases for mode tables, timing values, color
conversion, and signal-processing edge cases.
