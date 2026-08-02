# RSSSTV Architecture

This document defines the intended architecture of the Rust implementation and
maps the current codebase onto that design. It is both a guide for new work and
a record of which architectural pieces already exist.

The architecture of the original MMSSTV application is described in
[mmsstv.md](mmsstv.md). See [mmsstv-porting.md](mmsstv-porting.md) for the
mapping from the original implementation to the proposed Rust boundaries,
[mmsstv-dsp.md](mmsstv-dsp.md) for original DSP details, and
[sstv-formats.md](sstv-formats.md) for mode and timing data. The portable
transmit overlay format is described in [template-design.md](template-design.md).

## Design Goals

RSSSTV aims to preserve the signal-processing and protocol behavior of MMSSTV
without preserving its Win32/VCL structure. The design should have these
properties:

- The reusable SSTV core is independent of UI, audio devices, radio control,
  logging, persistence, and operating-system APIs.
- Receive and transmit processing can run deterministically on in-memory data
  for tests and offline tools.
- Streaming boundaries have explicit ownership, timing, and error contracts.
- Stateful processors own their configuration and mutable state; there are no
  equivalents of MMSSTV's global `sys`, `SSTVSET`, or main-form state.
- Mode definitions, raster timing, and color conversion are shared by transmit
  and receive paths.
- Platform and application layers depend on the core, never the reverse.
- Real-time integrations use bounded queues or equivalent backpressure instead
  of implicitly shared buffers and counters.

The smallest portable core should remain usable without an application or live
audio backend. Where practical, core crates should support `no_std` with
allocation rather than requiring the standard library.

## Target Layers

The target architecture separates numerical processing, SSTV protocol logic,
platform integration, and application behavior.

| Layer | Responsibility | Current location |
| --- | --- | --- |
| DSP | Filters, FFT, Hilbert transforms, oscillators, PLL, tone detection, and frequency measurement | `rssstv-dsp` |
| Protocol model | Modes, identifiers, image geometry, raster descriptions, timing, frequencies, images, and color conversion | `rssstv-sstv` |
| Receive front end | PCM preprocessing, frequency demodulation, sync confidence, VIS/FSK detection, and AFC | `rssstv-demodulator`, `rssstv-fskid` |
| Receive decoder | Raster acquisition, clock estimation, synchronization, slant correction, and pixel reconstruction | `rssstv-sstv::rx` |
| Transmit encoder | Image-to-raster conversion, headers, VIS, scan lines, and identifiers | `rssstv-sstv::tx` |
| Modulator | Timed frequencies to PCM samples | Not implemented |
| Audio adapters | Platform-specific input and output streams | Not implemented |
| Integration | Composition of core stages for a particular environment | `decode-wav` for offline receive |
| Template composition | KDL scene parsing, variables, RGBA overlay rendering, and RGB composition | `rssstv-template` |
| Application | UI, configuration, history, template editing, logging, PTT, CAT, and orchestration | Not implemented; `rssstv` is a placeholder |

These are responsibility boundaries, not a requirement that every row become a
separate crate. Closely related protocol types currently live together in
`rssstv-sstv`; they should be split only when a concrete dependency or reuse
need justifies it.

## Target Data Flow

Receive and transmit are explicit pipelines with shared protocol and image
types.

```text
Receive:
AudioSource
  -> Preprocessor / Demodulator
  -> demodulated frequency and synchronization stream
  -> RxDecoder
  -> ImageSink

Transmit:
ImageSource
  -> TxEncoder
  -> timed frequency stream
  -> Modulator
  -> AudioSink
```

The processing stages should not know whether their source or sink is a device,
a file, a test vector, or another in-memory component. Platform adapters select
buffer sizes and scheduling without changing protocol behavior.

### Current Receive Flow

The complete receive integration currently available is the offline
`decode-wav` path:

```text
WAV file
  -> packetized first-channel normalized PCM
  -> rssstv-demodulator::Demodulator
  -> incremental demodulated blocks
  -> rssstv-sstv::RxDecoder
  -> staged global slant refinement
  -> BMP/JPEG/PNG image
```

`decode-wav` reads and processes PCM packets without retaining the complete WAV
or a separate complete demodulated array. Its packet size defaults to 1024 mono
samples and is configurable with `--packet-size`. Demodulation and raster
decoding run sequentially in one thread, while bounded staging may retain
demodulated samples for the optional whole-image refinement pass. There is no
live audio source yet.

### Current Transmit Flow

The portable transmit path currently stops at timed frequency events:

```text
rssstv-sstv::RgbImage
  -> rssstv-sstv::TxEncoder
  -> Iterator<Item = TimedTone>
  -> not implemented: Modulator
  -> not implemented: AudioSink
```

Tests use local tone synthesis where PCM is needed, but that code is not a
production modulation component.

## Core Contracts

### Images and Modes

`RgbImage` is the owned, row-major image exchanged with the codec. `Mode` and
`ModeSpec` provide protocol identifiers, transport geometry, active rows,
raster periods, signal bands, and support status. Raster descriptions and color
conversion are shared by transmit and receive processing so that family-specific
ordering is defined once.

The mode inventory contains metadata for all 43 MMSSTV modes. Raster encoding
and decoding are currently implemented for:

- Robot 36 and Robot 72.
- Scottie 1, Scottie 2, and Scottie DX.
- Martin 1 and Martin 2.
- PD50, PD90, PD120, PD160, PD180, PD240, and PD290.

The other 29 modes remain metadata-only. This includes AVT, SC2, Pasokon, MR,
MP, ML, Robot 24, monochrome Robot modes, and narrow modes. Unsupported
behavior is represented explicitly rather than silently approximated.

### Demodulated Receive Data

The boundary between demodulation and raster decoding consists of physical
sample positions plus parallel frequency and normalized synchronization-strength
samples. `DemodulatedBlock` enforces continuity and value validation at this
boundary.

Frequency and synchronization strength are causal detector outputs and remain at
the sample positions where they were produced. The synchronization envelope
lags the frequency-discriminator output, so a demodulator reports that calibrated
relative delay separately. The integration passes it through `RxConfig` so
raster acquisition, live phase correction, and staged slant refinement use the
frequency stream's time coordinate. The delay is converted with the physical
receive sample rate, independently of the estimated raster clock. Inputs whose
two streams are already aligned use a zero delay.

`RxDecoder` is stateful and streaming. It exposes acquisition, decoding,
completion, and stopped states, consumes an explicit prefix of each input block,
and reports typed events and errors. Its responsibilities include:

- Initial raster acquisition and effective sample-clock estimation.
- Family-specific RGB and luminance/chroma reconstruction.
- Stable live raster-phase correction.
- Optional automatic stop based on synchronization history.
- Optional bounded staging and deterministic whole-image reconstruction.

Staged refinement currently estimates one global sample rate and raster epoch.
Local raster warping is outside the implemented contract.

### Timed Transmit Data

`TxEncoder` owns an image and yields `TimedTone` values. Each value carries a
frequency, a protocol component, and an exact deadline relative to transmission
start. Deadlines, rather than rounded sample counts, preserve protocol timing
until a modulator chooses a physical sample rate.

The encoder currently emits conventional VIS framing and mode raster data. It
does not emit VOX framing, footers, station identification, or PCM samples.

### FSK Identification

`rssstv-fskid` keeps six-bit FSKID framing separate from audio tone detection.
`FskDecoder` consumes samples classified as mark, space, or ambiguous and
returns validated `FskId` values. The receive front end owns the 1900/2100 Hz
detectors and supplies those classifications.

Callsign records are implemented. Contest records, N-VIS events, and FSKID
transmission remain future work. See [mmsstv-fskid.md](mmsstv-fskid.md) for the
protocol definition.

## State, Ownership, and Concurrency

DSP and codec objects own all mutable processing state. Input data is borrowed
for the duration of a processing call or moved into an owning stage such as
`TxEncoder`. Configuration is passed to the subsystem that uses it instead of
being read from global application settings.

Core APIs do not create threads or select an asynchronous runtime. This keeps
them deterministic and allows applications to choose an execution model. A
future live pipeline should place bounded queues between independently scheduled
audio and codec stages, preserve sample positions across those queues, and make
overflow or backpressure behavior explicit.

The receive staging option is bounded by a caller-provided sample limit. It is a
deliberate offline or deferred-refinement facility, not an unbounded hidden
queue.

## Platform Boundary

The following concerns belong outside the portable DSP and SSTV core:

- Audio device enumeration, callbacks, and stream formats.
- Windows messages, handles, VCL controls, and other GUI toolkit types.
- PTT and CAT transports.
- Logging services and external logger integrations.
- History, template editing, settings persistence, and application file
  management.
- Thread scheduling and application-level queue policy.

Integration crates may adapt these facilities to core data types, but platform
types must not appear in reusable core APIs.

## Current Crate Structure

The workspace currently contains seven packages:

| Package | Architectural role | Current status |
| --- | --- | --- |
| `rssstv-dsp` | Portable numerical layer | Implemented |
| `rssstv-sstv` | Protocol model, images, transmit encoder, and receive decoder | 14 modes implemented |
| `rssstv-fskid` | FSKID protocol decoder | Callsign receive implemented |
| `rssstv-demodulator` | Receive front end | Incremental conventional-VIS demodulation implemented |
| `rssstv-template` | Portable application-support layer | KDL parsing and SVG-backed RGBA rendering implemented |
| `decode-wav` | Offline receive integration | Implemented |
| `rssstv` | Application composition root | Placeholder only |

Their current dependency direction is:

```text
rssstv-dsp -----------+
rssstv-fskid ---------+-> rssstv-demodulator --+
rssstv-sstv ----------+                       +-> decode-wav
rssstv-fskid ----------------------------------+
rssstv-sstv -----------------------------------+
rssstv-sstv -----------------> rssstv-template

rssstv  (currently has no dependencies on the other packages)
```

`rssstv-dsp` and `rssstv-sstv` build as allocation-backed `no_std` crates by
default. `rssstv-fskid` is also `no_std`. Audio file and image format dependencies
remain in `decode-wav`, outside the portable core. `rssstv-template` is a
standard-library application-support crate: it depends on `rssstv-sstv` only at
the received-image and final RGB composition boundaries. It does not expose
SSTV modes to the template format or make the protocol crate depend on template
rendering.

## Current Implementation Detail

`rssstv-dsp` provides radix-2 FFT, windowed real spectra, FIR and IIR design and
processing, Hilbert transforms, zero-crossing frequency measurement, a
phase-continuous VCO, PLL frequency discrimination, and resonator tone
detection. The standalone FFT and PLL are not currently part of the WAV receive
path; that path uses a Hilbert phase-difference discriminator.

`rssstv-demodulator` provides a stateful `Demodulator` that accepts contiguous
normalized mono PCM packets and emits owned demodulated chunks with absolute
sample positions, a one-shot VIS mode event, and completed FSK identifiers. The
existing `demodulate` batch function is a convenience wrapper over that API. The
front end performs band-pass filtering, level normalization, VIS/FSK tone
detection, conventional VIS decoding, zero-crossing AFC measurement, and
Hilbert frequency discrimination. Its synchronization envelope is causal, with
its calibrated delay relative to the frequency output carried as metadata rather
than implemented by shifting the sample array. It requires at least a 6000 Hz
sample rate.

`decode-wav` composes the existing receive stages packet by packet. It uses the
first WAV channel, enables live raster synchronization and bounded in-memory
staging, performs global slant refinement, and saves BMP, JPEG, or PNG according
to the output extension.

`rssstv-template` strictly parses ordered KDL v2 layers, resolves frame-relative
geometry and caller-provided variables, PNG assets, received images, and fonts,
then generates static SVG for `resvg`. It returns a straight-alpha RGBA overlay
and can source-over composite that overlay into `rssstv-sstv::image::RgbImage`.
It does not select or prepare the background image and does not access history
or the filesystem implicitly.

## Planned Gaps

The architecture is not complete until the following boundaries have production
implementations:

- A modulator that converts `TimedTone` deadlines to phase-continuous PCM.
- Audio source and sink adapters with explicit buffering and backpressure.
- Transmit and receive raster processing for the remaining modes.
- Audio detection of extended VIS and N-VIS.
- FSKID, VOX framing, footer, and station-ID transmission.
- An application composition root and user interface.
- PTT, CAT, logging, history, template editing, and persistent configuration.
- Real-world received-audio regression fixtures.

These should extend the dependency structure above rather than placing platform
or application behavior into the core crates.

## Verification Strategy

Core behavior is tested with deterministic in-memory signals and images. The
test suite covers numerical primitives, mode metadata and timing, all currently
supported raster families, transmit/receive round trips, synchronization and
slant behavior, malformed streaming input, FSKID at multiple sample rates, and
a synthesized WAV-to-PNG integration path. Template tests cover strict KDL
validation, all initial layer kinds, caller-resolved PNG and receive images,
straight-alpha rendering, and RGB source-over composition.

Run the complete verification set from the workspace root:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build --workspace
```
