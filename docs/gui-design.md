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
| `rssstv` | Composition root: window, state, messages, views, worker supervision | Implemented for receive |
| `rssstv-audio` | Capture and playback adapters over the host audio API | Capture implemented; playback not implemented |

`rssstv-audio` splits an open device into a `Capture` handle and a `CaptureReader`. The handle keeps the stream alive on the thread that opened it, because host streams are not `Send` on every platform; the reader is `Send` and moves into the receive worker.

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
  -> single-slot snapshot mailbox
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

Capture overrun breaks the contiguity the demodulator requires. `Reading`
reports the gap, and the worker responds by discarding the demodulator and any
decoder and starting again, rather than decoding across a discontinuity and
producing a silently corrupt image.

Progressive image display uses the existing decoder API. The worker tracks
`RxDecoder::image_revision`, and when it changes, converts `RxDecoder::image`
into RGBA bytes and publishes an owned frame. The conversion runs on the worker
so the interface only wraps the buffer, without copying pixels. Publication is
throttled to at most 30 frames per second; the revision counter makes skipped
intermediate revisions harmless.

Slant correction is enabled by default. For a reception that starts while it is
enabled, the worker retains a bounded full-rate frequency/sync stream and
rebuilds the completed image with a fitted global raster rate and epoch. The
control does not replace live phase synchronization, which remains enabled
independently. Turning Slant on after a reception has started applies to the
next reception because the earlier samples were not staged.

Slant is corrected while the image is still arriving, as MMSSTV does, not only
after it finishes. A reception begins on the configured capture rate, exactly as
MMSSTV begins on its calibrated `SSTVSET.m_SampFreq`. The startup window spans
too few periods to estimate a better rate; fitting one there lands hundreds to
thousands of parts per million out on real signals, which is worse than the
capture rate the operator already has, and it visibly bends the first lines
before tracking can take over.

`RxConfig::live_slant` turns on that tracking. The decoder refits the rate from
the synchronization collected so far, smooths it over at most sixteen estimates,
and applies a correction once the error clears a threshold that tightens as
lines accumulate, redrawing the rows already decoded from the retained samples.
As in MMSSTV, the average covers however many estimates have been collected, so
a gross rate error is corrected from the first estimate rather than after the
window has filled. This mirrors MMSSTV's `AutoStopJob()` real-time adjustment;
see [mmsstv-dsp.md](mmsstv-dsp.md). Because it redraws, it requires
`Staging::Memory`.

A refit that runs faster than the previous estimate places the decoded units
later in the stream, sometimes past the audio received so far. The redraw
therefore covers as many leading units as the retained samples reach and the
remainder is decoded again as the audio arrives, rather than the correction
being rejected for lack of coverage.

Staged refinement stays as the more precise final pass, matching MMSSTV's
separate `CorrectSlant()`. With live tracking on it is a refinement of an
already-straight image rather than the only thing standing between the operator
and an unusable one.

A refit raster reaches slightly past the samples decoded live, so refinement
cannot run the moment the raster completes. The worker keeps feeding trailing
audio through `RxDecoder::stage_for_refinement` and retries
`RxDecoder::refine_staged` on a sample budget, treating
`InsufficientStagedData` as "wait for more audio" rather than as a failure. The
search for the next signal is held off until refinement resolves, because
restarting the demodulator would cut off the tail it is waiting for.

The corrected image therefore appears about a second after the raster fills,
once enough trailing audio has arrived. A live device supplies that tail on its
own, because it keeps delivering samples once the transmission has stopped.

`rssstv-audio` exposes `synthetic_capture`, a capture queue with no device
behind it. Recorded audio pushed through it drives the receive worker over
exactly the code path a live device uses, which is what makes the pipeline
testable without hardware.

`auto_stop` is left disabled. Its live synchronization scoring aborts genuine
receptions part way through, which loses both the remaining rows and the
refinement that would have corrected the slant. A reception whose signal simply
disappears is ended by a worker-side stall timeout instead.

Raster phase acquisition collects four recurring synchronization pulses, as
MMSSTV does, buffering at most five periods when the first post-VIS pulse is
incomplete. It then decodes from the retained first period. The interface
therefore starts publishing at the first row after the short acquisition
interval instead of waiting for a fixed fraction of the image.

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

The worker publishes into a single-slot mailbox rather than a queue. Writing
replaces the pending snapshot, carrying forward an image frame or error the
interface has not collected yet, so a newer meter-only snapshot cannot discard
a decoded frame. Because each snapshot fully describes the receive state, one
slot is sufficient, and it bounds the handoff by construction: an interface
that stops polling cannot make the worker accumulate megabyte frames.

The interface invalidates the canvas cache only when the raster, the detected
mode, or the decoded fraction actually changed, so an idle receiver does not
retessellate every frame.

A detected mode replaces the operator's selection only while automatic
detection is on. With it off, the dropdown selection stands.

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
and locale switching are interactive. Template and stock lists are loaded from
the application directories at startup; each list can be refreshed and its
directory can be opened in the platform file manager.

Receive is implemented end to end. Selecting a device opens a capture stream
and spawns the receive worker, which demodulates, detects the mode from VIS,
decodes the raster, and publishes snapshots. The interface adopts the newest
snapshot on each frame and draws the partially decoded image progressively.

Nothing simulated remains in the receive path. Mode, decoded rows, input level,
synchronization strength, decoded callsigns, and overrun counts all come from
the worker. When no reception is in progress, the canvas shows a blank raster
sized to the selected mode.

Transmit is not implemented. The transmit tab still shows a generated test
pattern, and controls belonging to the transmit pipeline are rendered without
an action so the layout stays reviewable without implying working transmit.

The mode dropdowns are already driven by `ModeSpec` support, so they list
exactly the modes the core can encode or decode.

## Prerequisites

The remaining implementation order is:

1. `rssstv-audio` playback: output streams and bounded queues for transmit.
2. Transmit worker: template rendering, `TransmissionEncoder`, and `Modulator`
   over the playback queue.
3. History: retaining completed receptions, and the receive controls that act
   on them.

The application storage directories and an empty default configuration file
are initialized at startup as described in [architecture.md](architecture.md).
Loading and saving configuration values, template editing, PTT, CAT, and
logging remain planned gaps.

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
