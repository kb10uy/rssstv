# MMSSTV Rust Porting Notes

This document describes proposed boundaries for porting the original MMSSTV
implementation in `original/mmsstv` to Rust. The architecture of the original
application is documented separately in [mmsstv.md](mmsstv.md). Detailed
references are available for the [DSP implementation](mmsstv-dsp.md) and
[supported SSTV formats](sstv-formats.md).

## Proposed Module Boundaries

The original source can be divided into the following conceptual Rust modules:

| Rust module | Original implementation |
| --- | --- |
| `dsp` | Numerical portions of `fir.cpp` and `Fft.cpp` |
| `sstv_modes` | Mode definitions, geometry, and `CSSTVSET` timing |
| `demodulator` | `CSSTVDEM`, PLL, FQC, Hilbert, VIS, sync, AFC, FSK |
| `rx_decoder` | `SyncSSTV()` and `DrawSSTV*()` logic from `Main.cpp` |
| `tx_encoder` | VIS/header generation and `LineXXX()` functions from `Main.cpp` |
| `modulator` | `CSSTVMOD` and `CVCO` |
| `audio` | Platform-independent audio stream interfaces replacing `CWave` |
| `image` | Owned RGB buffers and color-space conversion |
| `application` | UI, history, logging, PTT, CAT, and external integrations |

## Required Core Scope

The core cannot be reproduced by porting only `fir.cpp` and `sstv.cpp`.
Essential codec behavior currently located in `Main.cpp` includes:

- Initial receive raster synchronization.
- Live raster-phase correction and automatic stop decisions from sync history.
- Receive sample-clock estimation, staged-data resynchronization, and slant
  correction.
- Conversion of demodulated values to pixels.
- Mode-specific RGB and luminance/chroma ordering.
- Transmission line scheduling and timing.
- VIS and narrow-mode identification generation.
- Transmission queue backpressure and completion handling.

The reusable core therefore includes both the DSP classes and the protocol and
scan-conversion code currently embedded in the VCL main form.

## Target Pipelines

A suitable target architecture consists of two explicit pipelines:

```text
Receive:
AudioSource -> Preprocessor -> Demodulator -> RxDecoder -> ImageSink

Transmit:
ImageSource -> TxEncoder -> Modulator -> AudioSink
```

Mode descriptions and color conversions should be shared by both pipelines.
Platform audio, UI, PTT, logging, and radio control should depend on the core,
not be referenced by it.

## State and Concurrency

The original implementation exchanges demodulator pages, modulator frequencies,
and FFT pages through shared counters and indices without C++ atomics. The Rust
implementation should give these streams explicit ownership and use bounded
single-producer/single-consumer queues or equivalent synchronization.

Global settings should be split into subsystem-specific configuration values.
DSP and codec state should be owned by their processing objects rather than
accessed through equivalents of `sys`, `SSTVSET`, or the main form.

## Platform Separation

The following dependencies should remain outside the portable codec core:

- VCL controls, forms, timers, and bitmaps.
- WinMM audio callbacks and `WAVEHDR` buffers.
- Win32 window messages and handles.
- COM-port PTT and radio CAT control.
- Logger and custom drawing DLL interfaces.
- History, template editing, and application file management.

This separation allows the original signal-processing and protocol behavior to
be retained without carrying VCL, Win32, global-state, or main-form dependencies
into the Rust core.
