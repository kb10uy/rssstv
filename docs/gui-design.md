# RSSSTV GUI Design

This document defines the application composition root: the desktop user
interface, its state model, and the boundaries between the interface and the
existing core crates.

It refines the `application` layer left unspecified in
[architecture.md](architecture.md). The core contracts described there are
assumed and are not restated here. Mode and timing data comes from
[sstv-formats.md](sstv-formats.md); the transmit overlay format is described in
[template-design.md](template-design.md).

## Framework

The interface uses [iced](https://github.com/iced-rs/iced) 0.14.

iced was selected over GPUI for three reasons:

- Its built-in widget set covers nearly every control the interface needs:
  dropdowns, togglers, checkboxes, progress bars, scrollable lists, text inputs,
  and image views. GPUI provides layout and paint primitives but no such
  widgets, so it would require an additional third-party component library.
- Its Elm-style `state -> message -> update` structure matches the existing
  requirement that stateful processors own their configuration and mutable
  state, with no global mutable state.
- Text layout goes through `cosmic-text`, which provides consistent CJK shaping
  and font fallback across platforms.

The accepted costs are a single large `Message` type, which is mitigated by
nesting per-area message enums, and continued breaking changes across iced
`0.x` releases.

Reproducing a specific visual style is not a goal. The design mock defines
placement and information hierarchy only.

## Layer Placement

The interface is an application layer above the existing core crates. It does
not add SSTV or DSP behavior.

| Package | Role | Status |
| --- | --- | --- |
| `rssstv` | Composition root: window, state, messages, views, worker supervision | Interface shell implemented; no workers yet |
| `rssstv-audio` | Capture and playback adapters over the host audio API | Not implemented |

`rssstv-audio` is a new crate. It owns device enumeration, stream formats, and
callback scheduling, and exposes only normalized mono `f32` blocks with sample
positions. Keeping it separate preserves the rule that platform types must not
appear in reusable core APIs, and lets the offline `decode-wav` and
`encode-wav` integrations remain unaffected.

The interface depends on `rssstv-audio`, `rssstv-sstv`, `rssstv-demodulator`,
`rssstv-modulator`, `rssstv-fskid`, and `rssstv-template`. No core crate gains
a dependency on the interface.

## Execution Model

The user interface thread never runs demodulation, raster decoding, modulation,
or template rendering. Those stages run on dedicated workers that own their
processing state, and the interface observes them through bounded queues.

```text
Receive:
audio capture callback
  -> bounded PCM queue
  -> receive worker (Demodulator + RxDecoder)
  -> bounded snapshot queue
  -> iced Subscription
  -> update

Transmit:
update
  -> transmit worker (Renderer + TransmissionEncoder + Modulator)
  -> bounded PCM queue
  -> audio playback callback
```

Three properties follow from this split:

- The audio callbacks only move blocks across a queue and never allocate,
  block, or run protocol code.
- The interface never holds a `Demodulator`, `RxDecoder`, or `Modulator`, so
  `update` stays cheap and the core stages stay deterministic and testable
  without the interface.
- Queue overflow is an explicit, reported condition rather than unbounded
  growth. Dropped capture blocks surface as a receive warning.

### Receive Worker

The receive worker owns one `Demodulator` and, after VIS detection, one
`RxDecoder`. It reproduces the composition already proven in
`decode-wav`, with two differences: input
arrives from a live queue instead of a WAV reader, and the worker publishes
intermediate snapshots instead of only a final result.

The worker computes the input level itself from the raw capture block before
demodulation. Level metering is a user-interface concern, not a protocol one,
so `Demodulator` gains no accessor for it.

Progressive image display uses the existing decoder API. The worker tracks
`RxDecoder::image_revision`, and when it changes, converts
`RxDecoder::image` into RGBA bytes and publishes an owned handle. The
conversion runs on the worker so the interface only receives display-ready
data. Publication is throttled to at most 30 updates per second; the revision
counter makes skipped intermediate revisions harmless.

`RxEvent` values are mapped to interface state as follows:

| Event | Interface effect |
| --- | --- |
| `RasterAcquired` | Mode and effective sample rate become known; progress starts |
| `RowDecoded` | Row count advances; drives the progress bar |
| `PhaseAdjusted` | Recorded for diagnostics only |
| `Stopped` | Session ends with a synchronization-lost status |

Synchronization confidence shown in the interface is derived from recent
`DemodulatedChunk::sync_strength` values, which are already normalized to
`0.0..=1.0`.

### Transmit Worker

The transmit worker renders the selected template over the prepared background,
builds a `TransmissionEncoder`, and streams PCM through `Modulator` into the
playback queue, mirroring `encode-wav`.
Template rendering through `resvg` is not real-time and runs before the
transmission starts, not inside the streaming loop.

Composite preview rendering, which happens whenever the selected template or
stock image changes, runs as a one-shot task rather than on the interface
thread.

## State Model

State is grouped by the area that owns it. Nothing is shared implicitly between
groups.

```text
App
  tab: Tab                       // Receive | Transmit | History
  audio: AudioState              // device selection, capture status
  rx: RxState                    // live session, image handle, level, sync
  tx: TxState                    // selected mode, prepared frame, progress
  history: HistoryState          // stored receptions and current selection
  library: LibraryState          // template list, stock list, preview
  qso: QsoState                  // callsign, RSV, serial number
  locale: Locale
```

Tab switching only changes which view is built. It never tears down a live
receive session, so leaving the receive tab during reception does not lose the
image.

## Message Design

A single top-level enum dispatches to per-area enums. This keeps `update` a
shallow match that delegates, instead of one large flat match.

```rust
enum Message {
    TabSelected(Tab),
    Audio(AudioMessage),
    Rx(RxMessage),
    Tx(TxMessage),
    History(HistoryMessage),
    Library(LibraryMessage),
    Qso(QsoMessage),
}
```

Messages arriving from workers are distinguished from user interaction by their
variant, not by an ambient flag. For example, `RxMessage::SnapshotReceived`
originates from the receive subscription, while `RxMessage::ModeSelected`
originates from the mode dropdown.

Worker snapshots are coalesced. If several snapshots are queued when the
subscription is polled, only the newest is turned into a message, because each
snapshot fully describes the current receive state.

## View Composition

The mock decomposes into a fixed outer frame and one tab-dependent region. The
following table maps each area of the mock to its iced construction. Areas
marked shared are built once and reused across tabs.

| Area | Scope | Construction |
| --- | --- | --- |
| Menu bar | Shared | Button row; `iced_aw` menu if submenus become necessary |
| Tab selector | Shared | Button row with the active tab styled differently |
| Input device selector | Shared | `pick_list` over enumerated capture devices |
| Main image view | Per tab | `canvas`; see below |
| Receive action bar | Receive | Buttons, `checkbox`, mode and size text |
| Transmit action bar | Transmit | Buttons including the transmit trigger |
| History action bar | History | Navigation buttons and record metadata |
| Input level meter | Shared | `progress_bar`; `canvas` if threshold marks are wanted |
| Sync indicator | Shared | Colored dot plus label |
| Mode panel | Shared | `toggler` for automatic detection plus `pick_list` |
| DSP panel | Shared | Three toggle buttons |
| QSO panel | Shared | `text_input` for callsign, RSV, and serial number |
| Template list | Shared | `scrollable` of selectable rows |
| Stock image list | Shared | `scrollable` of selectable rows with thumbnails |
| Composite preview | Shared | `image` plus edit and set-for-transmit actions |
| Status bar | Shared | Text row |

### Main Image Canvas

The main image view is a `canvas` rather than an `image` widget. The received
raster is only one of several things drawn in that area, and the rest cannot be
expressed by a plain image view:

- The undecoded region below the current scan position, and the boundary line
  marking that position.
- The status badge, progress indicator, and any diagnostic overlay, composited
  in the same pass and in the same coordinate space as the raster.
- Letterboxing of the mode's aspect ratio inside a resizable area, without
  distorting the raster.

Drawing goes through `canvas::Image`, whose `filter_method` is set to nearest
neighbor. SSTV rasters are small and are displayed at non-integer scales;
linear filtering would blur individual scan lines and hide exactly the artifacts
an operator needs to see when judging synchronization.

The canvas keeps a `canvas::Cache` that is invalidated on
`RxDecoder::image_revision` change or overlay state change, so a redraw is not
performed per frame. Because the canvas already owns pointer input and view
transforms, later additions such as zoom, pan, and slant preview extend it
rather than replacing it.

The composite preview and the list thumbnails have no overlays and remain plain
`image` widgets.

Two mock behaviors are deliberately reduced relative to the original MMSSTV
interface, as recorded in the reference breakdown:

- Receive analysis shows an input level and a valid-signal indicator. There is
  no spectrum display or waterfall.
- The mode panel defaults to automatic VIS detection, with manual selection
  available from a dropdown rather than a column of per-mode buttons.

The mode dropdown lists only modes whose `ModeSpec` reports the relevant
direction as supported, so unimplemented modes are never selectable.

## Internationalization

User-visible strings are external from the first commit. English is the default
locale and Japanese is the second. Retrofitting extraction later is more
expensive than doing it now, and the string volume is small.

- Strings are stored as [Fluent](https://projectfluent.org/) resources, one
  file per locale, loaded through `fluent-bundle`.
- Strings are never assembled by concatenation. Values that vary are passed as
  Fluent arguments so translations control word order.
- Protocol identifiers are not translatable. Mode names such as `Scottie 2` and
  `PD120`, and the terms RSV, FSKID, AFC, LMS, and VIS, are rendered from
  `ModeSpec` and related core types directly.
- Numeric and time formatting follows the active locale; frequency, sample
  rate, and dBFS values keep their conventional units.
- The default font must resolve Japanese glyphs. `cosmic-text` performs system
  font fallback, so this is a matter of verifying rendering per platform rather
  than embedding a CJK font.

Locale selection is explicit in application configuration, defaulting to the
system locale when it matches an available translation.

## Current Implementation

The `rssstv` shell implements the state model, message dispatch, and view
composition described above. Tabs, mode selection, DSP toggles, QSO fields,
locale switching, and the template and stock lists are interactive.

Nothing in the shell performs SSTV processing. In place of the receive worker,
a simulation advances a decoded fraction, an input level, and a synchronization
strength on a fixed cycle, and the main canvas draws a generated test pattern
sized to the selected mode. Controls whose behavior belongs to the audio
boundary are rendered without an action, so the layout is reviewable without
implying working transmit or receive.

The mode dropdowns are already driven by `ModeSpec` support, so they list
exactly the modes the core can encode or decode.

## Prerequisites

The interface cannot perform live reception or transmission until the audio
boundary exists. The remaining implementation order is:

1. `rssstv-audio`: device enumeration, capture and playback streams, bounded
   queues, and overflow reporting.
2. Worker integration: receive and transmit workers over the real audio
   adapters, replacing the simulation and enabling the pending controls.

Configuration persistence, template editing, PTT, CAT, and logging remain out
of scope for this document and are still listed as planned gaps in
[architecture.md](architecture.md).

## Verification Strategy

Core behavior stays covered by the existing deterministic tests. The interface
adds:

- `update` tests that drive message sequences against the state model without
  constructing a window, including tab switching during an active receive
  session and queue-overflow reporting.
- Worker tests that feed recorded PCM through the receive worker and assert the
  published snapshot sequence, reusing the synthesized signals already used by
  the offline integrations.
- A locale test asserting that every message key present in the default locale
  exists in each additional locale.
