# MMSSTV Architecture

This document summarizes the architecture of the original MMSSTV source code in
`original/mmsstv`.

## Overview

MMSSTV is a Win32 desktop application written for Borland/Embarcadero C++Builder
and the Visual Component Library (VCL). It is not organized as a standalone DSP
library with a separate user interface. Instead, the main VCL form coordinates
the application and also implements essential parts of the SSTV codec.

At a high level, the application consists of:

```text
Mmsstv.exe
  |-- TMmsstv                 UI, application control, and image scan conversion
  |-- TSound                  Real-time audio worker thread
  |   |-- CWave               WinMM audio input and output
  |   |-- CSSTVDEM            Demodulation, synchronization, and mode detection
  |   |-- CSSTVMOD            Frequency-stream modulation and PCM generation
  |   |-- CLMS / CNotch       Optional input preprocessing
  |   `-- CFFT                Spectrum collection and FFT processing
  |-- CDraw hierarchy         Template and image composition
  |-- CLogFile / CLogLink     QSO logging and logger integration
  |-- CComm / CCradio         PTT and radio control
  `-- VCL dialogs and viewers
```

The main architectural characteristic is tight coupling through global state,
raw pointers, VCL types, and direct references to the main form.

## Project and Build Structure

The application entry point is `WinMain` in `original/mmsstv/Mmsstv.cpp:130`.
It prevents duplicate instances unless `-Z` is present, initializes VCL, creates
the global `TMmsstv` form, and starts the VCL message loop.

The `USEUNIT` and `USEFORM` declarations in `Mmsstv.cpp:24` provide an effective
manifest of the application modules. The principal project files are:

| File | Purpose |
| --- | --- |
| `Mmsstv.cbproj` | Current C++Builder VCL project |
| `Mmsstv.bpr` | Legacy C++Builder 5 project |
| `Mmsstv.cpp` | Application entry point and unit manifest |
| `*.cpp`, `*.h` | Implementations and declarations |
| `*.dfm` | VCL form resources |
| `jpeg/` | Statically compiled IJG JPEG implementation |

The tree also contains separate products that are not part of the main SSTV
processing path:

| Directory | Product |
| --- | --- |
| `JASTA/` | Contest and logging support application |
| `CItems/TextArt/` | Custom drawing item DLL |
| `CItems/TEXTBOX/` | Text box drawing item DLL |
| `CItems/QSLBox/` | QSL drawing item DLL |
| `CItems/PERIMG/` | Perspective image drawing item DLL |

## Main Application Controller

### `TMmsstv`

`TMmsstv`, declared in `Main.h:50` and implemented in `Main.cpp`, is both the
main window and the central application controller. `Main.cpp` is approximately
14,000 lines and contains much more than UI event handling.

Its responsibilities include:

- Managing the Sync, RX, History, TX, and Template pages.
- Starting and stopping reception and transmission.
- Converting demodulated samples into image pixels.
- Converting image pixels into timed SSTV frequency sequences.
- Generating VIS headers, narrow-mode identifiers, FSK ID, and CW ID.
- Maintaining receive history and image storage.
- Editing and compositing transmission templates.
- Coordinating logging, PTT, radio control, and repeater behavior.
- Updating auxiliary windows and external integrations.

The sound worker owns its demodulator and modulator by value. `TMmsstv` stores
non-owning pointers to those objects as `pDem` and `pMod`. It also owns many VCL
bitmaps, dialogs, viewers, and platform integration objects through raw pointers.

Application initialization occurs in two stages. The `TMmsstv` constructor
creates bitmaps, defaults, settings, and a suspended `TSound` instance. The
sound thread is resumed during the first form paint after log, callsign, locale,
country, and communication resources have been initialized.

### Global State

Much of the application state is process-global rather than explicitly owned by
a subsystem.

| Global | Purpose |
| --- | --- |
| `Mmsstv` | Main form instance |
| `sys` | Audio, UI, history, drawing, repeater, and transmission settings |
| `SSTVSET` | Current mode geometry and timing parameters |
| `Log` | QSO log storage |
| `LogLink` | External logger integration |
| `COMM` | Serial and PTT configuration |
| `RADIO` | Radio CAT configuration |
| `DrawPara` | Shared drawing parameters |

`SYSSET`, declared in `ComLib.h`, is especially broad. It combines settings that
would normally belong to audio, codec, UI, storage, and integration layers.

## Audio Processing

### `TSound`

`TSound` is a VCL `TThread` declared in `Sound.h:57`. It owns the real-time audio
and DSP objects:

- `CWave`
- `CSSTVDEM`
- `CSSTVMOD`
- `CLMS`
- `CNotch`
- `CFFT`
- `CWaveFile`

The central worker loop is `TSound::Execute()` at `Sound.cpp:274`. Input is read
in blocks, but demodulation and modulation are performed sample by sample.

The receive-side processing order is:

```text
Audio input block
  -> optional notch filter
  -> optional LMS filter or noise reducer
  -> CSSTVDEM::Do(sample)
  -> FFT collection
```

During transmission, the block contents are replaced with samples generated by
`CSSTVMOD::Do()` before being sent to the output device. The same worker also
performs RX/TX device switching and informs the audio backend of PTT state.

Although `TSound` is the main boundary between platform audio and DSP, it is not
independent of the UI. It calls methods on the global `Mmsstv` form during audio
reconfiguration and uses VCL and Win32 thread facilities.

### `CWave`

`CWave`, declared in `Wave.h:100`, wraps WinMM `waveIn` and `waveOut`. It manages
preallocated `WAVEHDR` FIFO buffers, callbacks, Windows events, and critical
sections.

The receive path is:

```text
WinMM driver
  -> WAVEHDR input FIFO
  -> WaveInProc callback
  -> input event
  -> CWave::InRead()
  -> TSound input buffer
```

The transmit side uses another `WAVEHDR` FIFO. `CWave::OutWrite()` converts the
worker's floating-point samples to PCM, submits buffers to WinMM, and waits for
completed output headers when necessary.

An alternative dynamically loaded MMW backend is supported through the API in
`mmw.h`. It can provide audio input, audio output, and PTT handling.

### Sampling Model

The hardware device is opened at the nominal `SampBase` rate, while `SampFreq`
can represent a calibrated logical rate. This distinction allows MMSSTV to
compensate for sound-card clock error and correct image slant without requesting
a nonstandard PCM rate from the device.

`InitSampType()` in `ComLib.cpp` selects a supported nominal rate, adjusts the
audio block size, and configures FFT decimation. Supported nominal rates include
8,000, 11,025, 12,000, 16,000, 18,000, 22,050, 24,000, 44,100, and 48,000 Hz.

## SSTV Mode Configuration

The mode enumeration is declared at `sstv.h:450`. It includes:

- Robot 8, 12, 24, 36, and 72.
- Scottie 1, 2, and DX.
- Martin 1 and 2.
- AVT.
- SC2 variants.
- PD50 through PD290.
- Pasokon P3, P5, and P7.
- MR, MP, and ML variants.
- Narrow MN and MC variants.

`CSSTVSET`, declared at `sstv.h:500`, stores the derived parameters for the
current receive and transmit modes. These include image dimensions, line time,
channel scan intervals, image-data offsets, sync positions, AFC windows, and
logical sample rates.

Mode timing and image geometry are calculated in `sstv.cpp`. They are consumed
by both the demodulator and the scan conversion code in `Main.cpp`.

## Receive Pipeline

The complete receive pipeline is:

```text
Audio device
  -> CWave input FIFO
  -> TSound preprocessing
  -> CSSTVDEM frequency demodulation
  -> demodulated sample and sync-strength page ring
  -> TMmsstv synchronization and scan conversion
  -> RGB image bitmap
  -> display and receive history
```

### Demodulation

`CSSTVDEM`, declared at `sstv.h:593`, converts audio samples into normalized
frequency values and synchronization measurements. Its responsibilities include:

- Input filtering and level normalization.
- AGC and clipping detection.
- FM discrimination.
- 1200 Hz and 1900 Hz synchronization detection.
- VIS state-machine decoding.
- AVT synchronization.
- Narrow-mode FSK identification.
- Automatic frequency control.
- FSK ID decoding.
- Repeater signal detection.
- Buffering demodulated data for scan conversion.

The FM discriminator is selectable:

| Type | Class | Method |
| --- | --- | --- |
| PLL | `CPLL` | Tracks instantaneous frequency with a VCO loop |
| Zero crossing | `CFQC` | Measures interpolated half-periods |
| Hilbert | `CHILL` | Differentiates unwrapped analytic-signal phase |

Tone detection uses narrow IIR resonators for frequencies including 1080, 1200,
1320, 1900, and 2100 Hz. Periodic line synchronization is tracked by `CSYNCINT`,
which compares recent sync intervals with expected line periods and tolerates
missed pulses by checking integral multiples.

### Demodulated Data Buffering

`CSSTVDEM` publishes two parallel arrays:

| Buffer | Contents |
| --- | --- |
| `m_Buf` | Normalized demodulated image-frequency samples |
| `m_B12` | Sync detector strength, normally 1200 or 1900 Hz |

The arrays form a 24-page ring, defined by `SSTVDEMBUFMAX` at `sstv.h:592`. The
sound thread produces pages, while the main VCL thread consumes them through the
shared read and write page indices.

### Synchronization and Image Scan Conversion

The demodulator does not directly produce an image. `TMmsstv::DrawSSTV()` at
`Main.cpp:4498` drains completed pages and dispatches mode-specific scan
conversion.

Initial raster alignment is performed by `SyncSSTV()`. It combines early sync
measurements modulo the expected line period, finds the strongest sync position,
and establishes the sample offset for the image raster.

The scan converter then maps sample position to line and pixel coordinates. It
handles the channel layout for each mode family, including:

- Direct RGB modes such as Martin and Scottie.
- Robot luminance and color-difference modes.
- PD two-line luminance with shared chroma.
- MR, MP, and ML layouts.
- Monochrome modes.
- Narrow modes.

Y/R-Y/B-Y data is converted to RGB before being written directly into a VCL
`TBitmap`. Optional timing, black-level, and differentiation adjustments are
also implemented in this section of `Main.cpp`.

Receive decoding is therefore divided between `sstv.cpp` and `Main.cpp`.

## Transmit Pipeline

The complete transmit pipeline is:

```text
Source image and template
  -> composed TX bitmap
  -> mode-specific LineXXX encoder
  -> timed frequency queue
  -> CSSTVMOD VCO and filtering
  -> PCM blocks
  -> CWave output FIFO
  -> audio device
```

### Image Preparation

`TMmsstv::MakeTxBitmap()` composites the source image and optional template into
the transmission bitmap. It performs mode-dependent sizing, clipping, stretching,
and transparent template drawing using VCL bitmap operations.

### Header and Line Generation

`TMmsstv::ToTX()` selects the mode, initializes the modulator queue, writes the
VOX/header sequence, writes standard VIS or narrow-mode identification, asserts
PTT, and asks the sound thread to enter TX mode.

Mode-specific line encoders are implemented in `Main.cpp:6088` and following.
Examples include `LineR36`, `LineR72`, `LineSCT`, `LineMRT`, `LinePD`, `LineMR`,
`LineMP`, `LineMN`, and `LineMC`.

`SendSSTV()` at `Main.cpp:6541` keeps the modulator queue filled and dispatches
the appropriate line encoder for the active mode. It also appends footer tones,
FSK ID, CW ID, or stored audio and waits for the final WinMM blocks to complete
before returning to receive mode.

Normal image values map linearly to 1500-2300 Hz. Narrow modes use 2044-2300 Hz.

### Modulation

`CSSTVMOD`, declared at `sstv.h:775`, owns a circular queue of frequency samples.
The main thread writes timed frequencies into the queue, and the sound thread
consumes them with `CSSTVMOD::Do()`.

The modulation path is:

```text
Queued frequency
  -> optional smoothing
  -> table-driven sine VCO
  -> output gain
  -> output band-pass filter
  -> PCM sample
```

Queue writes use a floating-point duration accumulator so that non-integral
sample durations remain accurate across successive protocol elements.

## DSP Support Modules

### `fir.cpp`

`fir.cpp` implements most reusable signal-processing primitives:

- FIR design and execution.
- Kaiser-windowed low-pass, high-pass, band-pass, and band-stop filters.
- Hilbert transformer design.
- IIR coefficient generation and cascaded execution.
- Narrow resonators.
- LMS adaptive filtering and noise reduction.
- FIR notch filtering.

The numerical algorithms share these files with VCL-based filter response
drawing and dependencies on application headers.

### `Fft.cpp`

`CFFT` collects 2048-sample pages from the sound thread. The main thread computes
and displays spectrum data. The handoff depends on `HWND`, `PostMessage`, and a
custom Windows message.

## Image Editing and Templates

The drawing model begins with `CDraw` at `Draw.h:109`. Derived classes include
line, box, title, text, picture, OLE, external-library, and group objects.

`CDrawGroup` owns drawing items and manages selection, layer order, macros,
serialization, and template loading and saving. `CDrawLib` dynamically loads
custom item DLLs and calls their exported `mcm*` interface.

Image I/O supports BMP, JPEG, and WMF. The bundled IJG JPEG implementation is
compiled directly into the executable and bridged to VCL bitmaps by code under
`jpeg/` and `ComLib.cpp`.

## Logging and Platform Integration

The major integration components are:

| Component | Responsibility |
| --- | --- |
| `CLogFile` | Native QSO log, indexing, search, import, and export |
| `CLogLink` | External logger and Hamlog communication |
| `CMMLink` | Dynamically loaded logger plugin API |
| `CMMRadio` | Dynamically loaded radio plugin API |
| `CComm` | COM port and RTS/DTR/BREAK PTT control |
| `CCradio` | Built-in CAT polling for several radio families |

External integrations use DLL loading, Win32 window handles, `WM_COPYDATA`, COM
ports, and application-specific exported interfaces. PTT is deliberately outside
the DSP path and can be controlled through serial lines, radio control, logger
integration, or the MMW audio backend.

## Threading and Buffer Ownership

The runtime has three principal execution contexts:

| Context | Responsibilities |
| --- | --- |
| VCL main thread | UI, scan conversion, TX line generation, files, history |
| `TSound` worker | Audio I/O, preprocessing, per-sample demodulation/modulation |
| WinMM callbacks | Audio FIFO bookkeeping and event signaling |

WinMM buffer management uses critical sections and Windows events. Higher-level
data exchange is less explicit:

- The demodulator page ring is produced by `TSound` and consumed by `TMmsstv`.
- The modulator frequency queue is produced by `TMmsstv` and consumed by
  `TSound`.
- FFT pages are produced by `TSound` and consumed by the VCL thread.

These structures use shared counters and indices without C++ atomics or explicit
memory barriers. They rely on a single-producer/single-consumer access pattern
and the behavior of the original x86 Windows environment.

## File Formats and Persistent Data

MMSSTV uses several application-specific formats in addition to common images.

The MMV audio format used by `CWaveFile` is not RIFF/WAV. It consists of a small
header containing a signature and sample-rate type followed by signed 16-bit PCM.
MMV data can be recorded from the receive path, replayed into the demodulator, or
inserted directly into a transmission.

Demodulated image and sync samples can also be stored temporarily for later
redraw, resynchronization, and slant correction. Receive history is persisted as
rotating BMP or JPEG files plus metadata.
