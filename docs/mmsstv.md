# MMSSTV Architecture

This document describes the overall structure of the original MMSSTV
application in `original/mmsstv`. Signal-processing details and supported SSTV
formats are documented separately:

- [MMSSTV DSP Implementation](mmsstv-dsp.md)
- [SSTV Formats Supported by MMSSTV](sstv-formats.md)
- [MMSSTV Rust Porting Notes](mmsstv-porting.md)

## Overview

MMSSTV is a Win32 desktop application written for Borland/Embarcadero
C++Builder and the Visual Component Library (VCL). It is not structured as a
standalone SSTV library with a separate user interface. The main VCL form owns
most application resources and also performs receive raster conversion and
transmit line generation.

```text
Mmsstv.exe
  |-- TMmsstv                 UI, orchestration, and image scan conversion
  |-- TSound                  Real-time audio worker
  |   |-- CWave               WinMM/MMW audio input and output
  |   |-- CSSTVDEM            Demodulation and receive detection
  |   |-- CSSTVMOD            Frequency-to-PCM modulation
  |   |-- CLMS / CNotch       Optional receive preprocessing
  |   `-- CFFT                Spectrum collection
  |-- CDraw hierarchy         Template and image composition
  |-- CLogFile / CLogLink     QSO logging and logger integration
  |-- CComm / CCradio         PTT and radio control
  `-- VCL dialogs and viewers
```

The implementation is tightly coupled through global state, raw pointers, VCL
types, Windows handles, and direct references to the main form.

## Project Structure

The entry point is `WinMain` in `original/mmsstv/Mmsstv.cpp:130`. It checks for
an existing MMSSTV window unless `-Z` is specified, initializes VCL, creates the
global `TMmsstv` form, and starts the VCL message loop.

The `USEUNIT` and `USEFORM` declarations in `Mmsstv.cpp:24` form an effective
module manifest.

| File | Purpose |
| --- | --- |
| `Mmsstv.cbproj` | C++Builder VCL project |
| `Mmsstv.bpr` | Legacy C++Builder 5 project |
| `Mmsstv.cpp` | Entry point and unit manifest |
| `*.cpp`, `*.h` | Implementations and declarations |
| `*.dfm` | VCL form resources |
| `jpeg/` | Statically compiled IJG JPEG implementation |

The source tree also contains independent products:

| Directory | Product |
| --- | --- |
| `JASTA/` | Contest and logging support application |
| `CItems/TextArt/` | Custom drawing item DLL |
| `CItems/TEXTBOX/` | Text box drawing item DLL |
| `CItems/QSLBox/` | QSL drawing item DLL |
| `CItems/PERIMG/` | Perspective image drawing item DLL |

## Main Form

`TMmsstv`, declared in `Main.h:50` and implemented in the approximately
14,000-line `Main.cpp`, is both the main window and the application controller.
It manages:

- Sync, RX, History, TX, and Template pages.
- Receive and transmit state transitions.
- Receive raster synchronization and bitmap generation.
- Transmit image preparation and mode-specific line scheduling.
- VIS, FSK ID, and CW ID generation.
- Image history and temporary receive storage.
- Template editing and image composition.
- Logging, PTT, CAT, repeater, and external application integration.
- Auxiliary viewers and dialogs.

The sound worker owns `CSSTVDEM` and `CSSTVMOD` by value. `TMmsstv` keeps
non-owning `pDem` and `pMod` aliases and owns many VCL bitmaps, dialogs, and
platform integration objects through raw pointers.

Startup occurs in two stages. The form constructor initializes settings,
bitmaps, history, templates, and a suspended `TSound`. The sound thread is
resumed during the first form paint after locale, callsign, country, log, and
communication resources have been initialized (`Main.cpp:2314`).

## Global State

Application state is distributed across process-global objects rather than
owned by explicit subsystems.

| Global | Purpose |
| --- | --- |
| `Mmsstv` | Main form instance |
| `sys` | Audio, UI, drawing, repeater, history, and TX settings |
| `SSTVSET` | Current SSTV mode geometry and timing |
| `Log` | QSO log storage |
| `LogLink` | External logger integration |
| `COMM` | Serial and PTT configuration |
| `RADIO` | Radio CAT configuration |
| `DrawPara` | Shared drawing parameters |

`SYSSET` in `ComLib.h` combines settings that would normally belong to audio,
codec, UI, persistence, and integration layers. Several nominally lower-level
modules include `Main.h` or `ComLib.h`, producing circular dependencies around
the main form and common globals.

## Runtime Data Flow

### Receive

```text
Audio device
  -> CWave input FIFO
  -> TSound preprocessing
  -> CSSTVDEM
  -> demodulated frequency and sync page ring
  -> TMmsstv::SyncSSTV / DrawSSTV
  -> VCL bitmap
  -> display and history
```

`CSSTVDEM` stops at demodulated frequency and synchronization data. The main
form drains its 24-page ring and performs the mode-specific conversion to RGB
pixels in `TMmsstv::DrawSSTV()` at `Main.cpp:4498`. It also owns initial
multi-line raster synchronization, live phase correction and automatic stop
decisions, sample-clock estimation from sync drift, and staged-data redraw for
resynchronization and slant correction. These operations are part of the
effective receive DSP even though they reside in `Main.cpp` rather than
`sstv.cpp`.

### Transmit

```text
Source image and template
  -> composed TX bitmap
  -> mode-specific LineXXX functions
  -> CSSTVMOD frequency queue
  -> VCO and output filtering
  -> CWave output FIFO
  -> audio device
```

The main form generates protocol headers and image scan lines. `SendSSTV()` at
`Main.cpp:6541` keeps the frequency queue filled. The sound thread consumes one
frequency command per PCM sample through `CSSTVMOD::Do()`.

The split means that `sstv.cpp` is not the complete SSTV engine. Essential RX
and TX protocol behavior is embedded in `Main.cpp`.

## Threading and Buffering

The application has three principal execution contexts:

| Context | Responsibilities |
| --- | --- |
| VCL main thread | UI, raster decoding, TX line generation, files, history |
| `TSound` worker | Audio I/O, preprocessing, demodulation, modulation |
| WinMM callbacks | Audio FIFO bookkeeping and event signaling |

WinMM input and output FIFOs use Windows events and critical sections. The
higher-level queues are shared without C++ atomics or explicit memory barriers:

- `CSSTVDEM` pages are produced by `TSound` and consumed by `TMmsstv`.
- `CSSTVMOD` frequencies are produced by `TMmsstv` and consumed by `TSound`.
- FFT pages are produced by `TSound` and consumed by the VCL thread.

These queues rely on single-producer/single-consumer access and practical x86
memory behavior. The UI timer polls most background state. High-priority FFT
collection instead posts a custom Windows message to the main window.

## Image and Template System

The drawing hierarchy begins with `CDraw` at `Draw.h:109`. Derived classes
represent lines, boxes, titles, text, pictures, OLE objects, external-library
items, and groups.

`CDrawGroup` owns items and manages selection, layer order, macros,
serialization, and template loading and saving. `CDrawLib` loads custom item
DLLs and invokes their exported `mcm*` interface.

Image I/O supports BMP, JPEG, and WMF. The bundled IJG JPEG sources are compiled
into the executable and bridged to VCL bitmaps through `jpeg/` and `ComLib.cpp`.

## Logging and Platform Integration

| Component | Responsibility |
| --- | --- |
| `CLogFile` | Native QSO log, indexing, search, import, and export |
| `CLogLink` | Hamlog and external logger communication |
| `CMMLink` | Dynamically loaded logger plugin API |
| `CMMRadio` | Dynamically loaded radio plugin API |
| `CComm` | COM port and RTS/DTR/BREAK PTT control |
| `CCradio` | Built-in CAT polling for several radio families |

External integrations use DLL loading, `WM_COPYDATA`, Win32 window handles,
COM ports, and application-specific exported interfaces. PTT can be controlled
through serial lines, radio control, logger integration, or the MMW audio
backend.

## Persistent Data

MMSSTV uses common image formats and several application-specific formats.
`CWaveFile` reads and writes MMV audio: a small signature and sample-rate header
followed by signed 16-bit PCM, rather than a RIFF/WAV container. MMV data can be
recorded from RX, replayed into the demodulator, or inserted into TX.

Demodulated frequency and sync samples can be staged in memory or in
`Sound.tmp` for redraw, resynchronization, and slant correction. Receive history
is persisted as rotating BMP or JPEG files with metadata.

## External Dependencies

The main application depends on:

- Borland/Embarcadero VCL and RTL.
- Win32 and WinMM APIs.
- Windows shell, OLE, serial, event, and window messaging facilities.
- The bundled IJG JPEG implementation.
- Optional MMW audio, MML logger, MMRP radio, EXTFSK, and drawing-item DLLs.

The original source is consequently both a protocol reference and a record of
application behavior, but its subsystem boundaries do not correspond directly
to its files or classes.
