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
The desktop application layer, its audio boundary, and its user interface are
described in [gui-design.md](gui-design.md).

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
| Modulator | Timed frequencies to PCM samples | `rssstv-modulator` |
| Audio adapters | Platform-specific input and output streams | `rssstv-audio`; capture implemented, playback not implemented |
| Integration | Composition of core stages for a particular environment | `decode-wav` and `encode-wav` |
| Template composition | KDL scene parsing, variables, RGBA overlay rendering, and RGB composition | `rssstv-template` |
| Application | UI, configuration, history, template editing, logging, PTT, CAT, and orchestration | `rssstv` receive interface; designed in [gui-design.md](gui-design.md) |

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

A live receive path also exists in `rssstv`, where the same stages run on a
worker thread fed by `rssstv-audio` instead of a WAV reader.

`decode-wav` reads and processes PCM packets without retaining the complete WAV
or a separate complete demodulated array. Its packet size defaults to 1024 mono
samples and is configurable with `--packet-size`. Demodulation and raster
decoding run sequentially in one thread, while bounded staging may retain
demodulated samples for the optional whole-image refinement pass. There is no
live audio source yet.

### Current Transmit Flow

The complete offline transmit integration is the `encode-wav` path:

```text
background BMP/JPEG/PNG
  -> cover resize and center crop to mode dimensions
KDL template + ${mycall} + background as rximage
  -> RGBA overlay and RGB composition
  -> rssstv-sstv::TransmissionEncoder
  -> VOX + VIS + raster + footer + FSKID + trailing silence
  -> rssstv-modulator::Modulator
  -> packetized normalized PCM
  -> 48 kHz mono 16-bit WAV
```

`rssstv-modulator` fills caller-owned output blocks and never retains the
complete PCM stream. It preserves oscillator phase across tone changes, treats
zero frequency as exact silence, and converts absolute picosecond deadlines to
sample endpoints without accumulating per-tone rounding error.

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
the sample positions where they were produced. The synchronization envelope is
used to find a pulse, never to time one. Its lag behind the frequency stream
runs to several milliseconds, and because the envelope is a ratio against
competing tone detectors, its weighted center also moves with the picture tones
either side of the pulse. Both errors would displace the whole picture
horizontally.

Every measured sync center is therefore refined on the frequency stream, where
the pulse is the window of one sync duration whose mean frequency is lowest.
Sliding a window of the known length beats reading edges off a threshold on two
counts: the discriminator ripples at twice the tone it tracks, which breaks a
threshold into fragments, and a threshold crossing sits at a different point on
each flank whenever the tones either side of the pulse differ. Displacing the
window either way trades sync samples for higher ones, so its minimum is on the
pulse whatever surrounds it. The frequency stream's own group delay needs no
compensation at all: pixel windows are read from that same stream, so a raster
placed on a frequency-domain center samples every pixel where its content
actually is. Acquisition, live phase correction, and staged slant refinement all
work in that one time base.

`RxConfig` still carries the envelope's approximate lag, but only to place the
search window around a detected pulse, so a rough figure is enough. Inputs whose
two streams are already aligned use a zero delay. A pulse that the search window
cannot see whole — one cut short by the start or the end of the retained samples
— has no usable center and is left out of the fit rather than pulling it.

`RxDecoder` is stateful and streaming. It exposes acquisition, decoding,
completion, and stopped states, consumes an explicit prefix of each input block,
and reports typed events and errors. Its responsibilities include:

- Initial raster phase acquisition from four recurring synchronization pulses,
  buffering at most five periods when the first post-VIS pulse is incomplete.
  The phase is averaged over those pulses, leaving out the first one because the
  buffer can begin part way through it.
- Skipping the leading raster units whose picture arrived before mode detection
  finished. Those rows stay blank and count as delivered, which is what the
  operator sees in MMSSTV too, instead of failing a reception over samples that
  were never received.
- Family-specific RGB and luminance/chroma reconstruction.
- Stable live raster-phase correction. A correction may move the raster
  backwards, so the working window keeps one raster period behind the unit being
  decoded rather than trimming to its start.
- Optional automatic stop based on synchronization history.
- Optional bounded staging and deterministic whole-image reconstruction.

Acquisition fixes the raster phase only. A reception starts on the configured
physical sample rate, as MMSSTV starts on its calibrated `SSTVSET.m_SampFreq`,
because the startup window is too short to estimate a rate that beats it.

Raster rate correction has two stages, as in MMSSTV. `RxConfig::live_slant`
refits the rate during decoding and redraws the rows already decoded from
retained samples, and `refine_staged` performs one more precise global fit after
completion. Both estimate a single global sample rate and raster epoch; local
raster warping is outside the implemented contract.

Refinement does not reuse synchronization observations collected through the
provisional live clock. It first acquires a stable clock from the first 32
staged periods, re-observes synchronization centers across the immutable staged
stream, and fits the final global clock from those observations. This keeps
early progressive display from biasing the completed-image slant correction.

### Timed Transmit Data

`TxEncoder` owns an image and yields `TimedTone` values. Each value carries a
frequency, a protocol component, and an exact deadline relative to transmission
start. Deadlines, rather than rounded sample counts, preserve protocol timing
until a modulator chooses a physical sample rate.

`TxEncoder` emits conventional VIS framing and mode raster data.
`TransmissionEncoder` wraps it with MMSSTV's built-in conventional VOX framing,
a 300 ms footer, a validated callsign FSKID, and 500 ms of trailing silence.
PCM conversion remains a separate modulator responsibility.

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

The workspace currently contains ten packages:

| Package | Architectural role | Current status |
| --- | --- | --- |
| `rssstv-dsp` | Portable numerical layer | Implemented |
| `rssstv-sstv` | Protocol model, images, transmit encoder, and receive decoder | 14 modes implemented |
| `rssstv-fskid` | FSKID protocol encoding and decoding | Callsign transmit and receive implemented |
| `rssstv-modulator` | Timed-tone PCM modulation | Streaming phase-continuous modulation implemented |
| `rssstv-demodulator` | Receive front end | Incremental conventional-VIS demodulation implemented |
| `rssstv-template` | Portable application-support layer | KDL parsing and SVG-backed RGBA rendering implemented |
| `decode-wav` | Offline receive integration | Implemented |
| `encode-wav` | Template-to-WAV transmit integration | Implemented |
| `rssstv-audio` | Host audio adapters | Capture implemented; playback not implemented |
| `rssstv` | Application composition root | iced interface with live receive; no transmit |

Their current dependency direction is:

```text
rssstv-fskid ----------------> rssstv-sstv
rssstv-dsp ------------------> rssstv-modulator
rssstv-audio ----------+
rssstv-demodulator ----+
rssstv-fskid ----------+
rssstv-sstv -----------+-> rssstv-modulator
rssstv-fskid ---------+-> rssstv-demodulator --+
rssstv-sstv ----------+                       +-> decode-wav
rssstv-fskid ----------------------------------+
rssstv-audio ----------+
rssstv-demodulator ----+
rssstv-fskid ----------+
rssstv-sstv -----------+-> rssstv-template
rssstv-fskid ----------+
rssstv-modulator ------+
rssstv-sstv -----------+-> encode-wav
rssstv-template -------+

rssstv-audio ----------+
rssstv-demodulator ----+
rssstv-fskid ----------+
rssstv-sstv -----------+-> rssstv
```

`rssstv-audio` is the platform audio boundary. It exposes normalized mono
`f32` samples with stream positions and keeps the host API out of its public
surface, so no core crate gains an audio dependency.

`rssstv-dsp` and `rssstv-sstv` build as allocation-backed `no_std` crates by
default. `rssstv-fskid` is also `no_std`. Audio file and image format dependencies
remain in `decode-wav` and `encode-wav`, outside the portable core.
`rssstv-template` is a
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
sample rate. The Hilbert transformer spans 100 Hz to 100 Hz below Nyquist, as
in MMSSTV; the preceding receive band-pass filter limits the SSTV audio band.

The live receive path does not resample or decimate PCM. Each captured mono
sample produces one demodulated frequency and synchronization value after VIS
detection. This matches MMSSTV's normal receive path; its rate-dependent Hilbert
phase span still emits one result per input sample, while its explicit
decimation is limited to displays and offline file conversion.

Raster conversion intentionally differs from MMSSTV's first-sample selection.
The Rust decoder averages the central five-eighths of the transmitted pixel
interval, leaving a narrow guard against adjacent-component contamination. This
is a deterministic anti-noise reconstruction policy rather than a downsampled
intermediate stream. Live phase correction adjusts the raster clock without
inserting or deleting demodulated samples.

`decode-wav` composes the existing receive stages packet by packet. It uses the
first WAV channel, enables live raster synchronization and bounded in-memory
staging, performs global slant refinement, and saves BMP, JPEG, or PNG according
to the output extension.

The GUI receive worker enables the same bounded staging by default. Its Slant
control applies to the next reception and performs a whole-image global
rate/epoch refinement at completion. Disabling it during a reception suppresses
that refinement; enabling it after reception has started cannot reconstruct the
missing unstaged prefix and therefore takes effect on the next reception.

`encode-wav` prepares the background at the selected mode's transport size,
renders the template with `${mycall}` and the background available as
`rximage`, and streams a complete framed transmission through
`rssstv-modulator` into `hound::WavWriter`. It uses bounded 1024-sample PCM
blocks rather than generating the complete waveform in memory.

`rssstv-template` strictly parses ordered KDL v2 layers, resolves frame-relative
geometry and caller-provided variables, PNG assets, received images, and fonts,
then generates static SVG for `resvg`. It returns a straight-alpha RGBA overlay
and can source-over composite that overlay into `rssstv-sstv::image::RgbImage`.
It does not select or prepare the background image and does not access history
or the filesystem implicitly.

## Application Storage

The desktop application uses the operating system's standard per-user
directories. Portable storage beside the executable is not supported.

| Content | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Configuration | `%APPDATA%\RSSSTV\config.toml` | `~/Library/Application Support/RSSSTV/config.toml` | `$XDG_CONFIG_HOME/rssstv/config.toml` |
| Templates and assets | `%APPDATA%\RSSSTV\templates`, `%APPDATA%\RSSSTV\assets` | `~/Library/Application Support/RSSSTV/templates`, `~/Library/Application Support/RSSSTV/assets` | `$XDG_DATA_HOME/rssstv/templates`, `$XDG_DATA_HOME/rssstv/assets` |
| User images | `Pictures\RSSSTV` | `~/Pictures/RSSSTV` | `$XDG_PICTURES_DIR/RSSSTV` |

The image directory contains `Stocks`, `Sent`, and `Received`. Images are kept
directly in those directories without year or month subdivisions. Templates
are KDL files stored directly in `templates`; reusable template images and
other resources are stored under `assets`.

At startup the application creates all of these directories and creates an
empty, valid `config.toml` when it does not already exist. Existing
configuration files are never replaced. Loading and saving configuration
values will be added with the configuration schema.

## Planned Gaps

The architecture is not complete until the following boundaries have production
implementations:

- Audio playback adapters and a transmit worker.
- Transmit and receive raster processing for the remaining modes.
- Audio detection of extended VIS and N-VIS.
- Contest FSK records, narrow N-VIS transmission, and optional CW identification.
- An application composition root and user interface.
- PTT, CAT, logging, history, template editing, and configuration persistence.
- Real-world received-audio regression fixtures.

These should extend the dependency structure above rather than placing platform
or application behavior into the core crates.

## Verification Strategy

Core behavior is tested with deterministic in-memory signals and images. The
test suite covers numerical primitives, mode metadata and timing, all currently
supported raster families, transmit/receive round trips, synchronization and
slant behavior, malformed streaming input, FSKID at multiple sample rates, and
a synthesized WAV-to-PNG integration path. The transmit integration test
encodes a complete Robot 36 WAV and decodes its image and FSKID. Template tests
cover strict KDL validation, all initial layer kinds, caller-resolved PNG and
receive images, straight-alpha rendering, and RGB source-over composition.

Run the complete verification set from the workspace root:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build --workspace
```
