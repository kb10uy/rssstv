# RSSSTV GUI Design

This document defines the application composition root: the desktop user
interface, its state model, and the boundaries between the interface and the
existing core crates.

It refines the `application` layer left unspecified in
[architecture.md](architecture.md). The core contracts described there are
assumed and are not restated here. Mode and timing data comes from
[sstv/modes.md](../sstv/modes.md); the transmit overlay format is described in
[template-design.md](template-design.md).

## Framework

The interface uses [egui](https://github.com/emilk/egui) 0.35 through eframe.
Its immediate-mode model keeps widget state in the application model while the
receive, composition, and transmit workers remain explicitly owned. Windows
and macOS use `muda` for the native menu bar; Linux renders the same menu model
inside the window. System fonts are discovered through `fontdb`, with egui's
bundled fonts retained as fallback.

## Platform Integration

Everything that only makes sense on one operating system lives in the
`platform` module, which selects one implementation file per target with
`#[path]`. Because every target supplies the same set of items, adding an
operation forces an answer on each platform before the build passes, even if
the answer is to do nothing. Font family names, revealing a directory in the
file manager, the window icon, and the Windows dark-mode opt-in are all
resolved there.

The menu bar is the one deliberate exception and stays in `menu`, because its
split is between two renderers of a shared model rather than between operating
systems.

A device that stops on its own is reported in front of the interface. The
stream error callback runs on the host's thread and cannot wait for anything,
so it leaves a `StreamFault` in a shared slot that the interface takes on its
next frame; the first report is kept, because a failing stream keeps failing
and the first one says why. A reroute and an overrun are deliberately not
faults: the stream survives both, and the samples an overrun dropped are
already counted separately. Taking a fault drops the capture handle and the
receive worker, enumerates the devices again so the menu offers what is
actually attached, and raises a modal naming the device, offering to open it
again or to dismiss the report.

Diagnostics are appended to a log file, and echoed to standard error when
there is a console to read it. A release build is linked as a Windows GUI
subsystem executable and has no console, so the reports that matter most would
otherwise be discarded exactly when nobody can see them. The file is kept
under the state directory — `XDG_STATE_HOME` on Linux and the local half of
the data directory elsewhere — rather than alongside the configuration, which
on Windows is the roaming profile and would synchronize a log describing
hardware the other machine does not have. It is started again once it passes a
megabyte, keeping one turnover.

Sleep is held off while a picture is moving. The application reports an
`Activity` of `Receiving` while a raster is being acquired or decoded, and
`Transmitting` while a transmission is priming, producing, or draining;
anything else is `Idle`. Windows answers with `SetThreadExecutionState`,
keeping the display on only while transmitting, which is short and attended.
An open device that nothing is arriving on stays `Idle`, so leaving the
application running does not hold sleep off indefinitely. Activity is the one
part of the platform surface reached through a trait rather than a free
function, because it follows application state and tests substitute a
recording implementation to assert what the interface asked for.

Only one copy runs at a time. The application holds audio devices open, and a
second copy silently failing to acquire them is harder to understand than not
opening at all, so a launch that finds the claim already taken asks the running
copy to come forward and exits. Windows takes the claim with a session-scoped
named mutex and publishes its window handle through a named shared section, so
the second launch can restore and raise the existing window; whether it reaches
the foreground is the platform's decision, and a refusal flashes the taskbar
button instead. Other platforms take the claim with a locked file and cannot
raise the running window yet. Windows also sets an explicit AppUserModelID, so
taskbar grouping and pinning do not follow the executable path and change
between a development build and an installed copy.

The window icon comes from the platform's own artwork store. On Windows the
build script embeds `rssstv/assets/icon.ico` as an executable resource, and the
application loads that resource back at startup, so the shell, the window, and
the task switcher all show one icon. Other platforms have no resource section,
so `rssstv/assets/icon.png` is compiled into the binary and decoded instead, as
does Windows when the build had no resource compiler. The executable also
carries a `VERSIONINFO` resource generated from `Cargo.toml`, so the version
Explorer reports cannot drift from the version the application reports.

Reproducing a specific visual style is not a goal. The design mock defines
placement and information hierarchy only.

## Layer Placement

The interface is an application layer above the existing core crates. It does
not add SSTV or DSP behavior.

| Package | Role | Status |
| --- | --- | --- |
| `rssstv` | Composition root: window, state, views, worker supervision | Receive and transmit implemented |
| `rssstv-audio` | Capture and playback adapters over the host audio API | Implemented |

`rssstv-audio` splits an open input device into a `Capture` and
`CaptureReader`, and an output device into a `Playback` and `PlaybackWriter`.
The device handles stay on the thread that opened them because host streams are
not `Send` on every platform. The queue halves move into the workers.

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
  -> interface polling

Transmit:
interface action
  -> transmit worker (Renderer + TransmissionEncoder + Modulator)
  -> bounded PCM queue
  -> audio playback callback
```

Three properties follow from this split:

- The audio callbacks only move blocks across a queue and never allocate,
  block, or run protocol code.
- The interface never holds a `Demodulator`, `RxDecoder`, or `Modulator`, so
  each frame stays cheap and the core stages stay deterministic and testable
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

A reception can start without a VIS header, from the spacing of the raster's
own synchronization pulses. How far that inference may reach follows the
automatic mode detection control, because a period can be matched by more than
one mode: with automatic detection on, any supported mode may be inferred, and
with it off only the selected mode is considered, so the match confirms the
operator's choice instead of overriding it. The scope is pushed to the worker
whenever it changes and is reapplied to every demodulator the worker builds,
including after a restarted search.

A reception whose signal stops arriving returns to signal search without
discarding its partial image. The worker watches for progress that has not
moved for the stall window, captures the decoder image, and starts a fresh
receive session. The captured frame stays on the canvas while the badge reads
as waiting for the next signal. When at least 65 percent of the rows were
decoded, the frame is also offered to automatic history; with automatic
history enabled it is stored in the received-image directory. Earlier
interruptions remain visible but are not retained in history. This is a normal
outcome and is not reported as an error on the status line.

History encoding is selectable between lossless WebP, PNG, and JPEG, with
lossless WebP as the default. Every format carries the same XMP packet: the
reception start time with its local UTC offset, the SSTV mode, and any FSKID
values decoded for that reception. After a complete raster, the worker waits
up to four seconds for the trailing FSKID before finalizing history, proceeding
immediately when an identifier arrives. The file name uses the reception time
through seconds and the mode. A second reception of the same mode in the same
second replaces that path instead of adding a sequence number.

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
see [mmsstv/dsp.md](../mmsstv/dsp.md). Because it redraws, it requires
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

`auto_stop` is enabled to match MMSSTV's live synchronization-loss decision.
When it fires, the worker handles the stopped decoder exactly like a stalled
input: the partial frame is retained, the 65-percent history rule is applied,
and a fresh signal search begins. The displayed row fraction is retained
separately from the new search's idle progress, so returning to the waiting
state does not conceal the partial frame. The worker-side stall timeout remains
as the fallback for a signal that stops producing decodable progress without
giving AutoStop further lines to score.

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

The interface shows raw input level as a single meter. Its fill is white when
no valid reception is active and uses the theme's green success color while
raster acquisition or decoding is in progress. It does not show a numeric
dBFS value or synchronization percentage.

### Transmit Worker

The composition worker renders the selected template over the prepared
background. Set for transmit freezes that immutable frame. The transmit worker
then builds a `TransmissionEncoder` and streams PCM through `Modulator` into the
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
  tab: Tab                       // Receive | Transmit
  audio: AudioState              // device selection, capture status
  rx: RxState                    // live session, image handle, level, sync
  tx: TxState                    // selected mode, prepared frame, progress
  library: LibraryState          // template list, stock list, preview
  qso: QsoState                  // callsign, RSV, serial number
  locale: Locale
```

Tab switching only changes which view is built. It never tears down a live
receive session, so leaving the receive tab during reception does not lose the
image.

## Event Handling

Widgets call narrow `App` methods for user actions. At the start of every frame,
the application polls the receive, composition, and transmit workers and adopts
their latest snapshots. Long-running work never executes as part of a widget
callback.

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
following table maps each area of the mock to its egui construction. Areas
marked shared are built once and reused across tabs.

| Area | Scope | Construction |
| --- | --- | --- |
| Menu bar | Shared | `muda` native menu or an egui in-window menu |
| Tab selector | Shared | Button row with the active tab styled differently |
| Input device selector | Shared | `pick_list` over enumerated capture devices |
| Main image view | Per tab | `canvas`; see below |
| Receive action bar | Receive | Buttons, `checkbox`, mode and size text |
| Transmit action bar | Transmit | Buttons including the transmit trigger |
| Input level meter | Shared | Signal-colored `progress_bar` |
| Mode panel | Shared | `toggler` for automatic detection plus `pick_list` |
| DSP panel | Shared | Three toggle buttons |
| QSO panel | Shared | `text_input` for callsign, RSV, and serial number |
| Template list | Shared | `scrollable` of selectable rows |
| Stock image list | Shared | `scrollable` of selectable rows with thumbnails |
| Composite preview | Shared | `image` plus edit and set-for-transmit actions |
| Status bar | Shared | Text row |

The input level meter, mode panel, and DSP controls share one bordered
container. The mode dropdown fills the container width and no tab shows helper
text below it.

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

- Receive analysis shows one input-level meter whose color indicates whether a
  valid signal is active. There is no numeric level, synchronization percentage,
  spectrum display, or waterfall.
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
- Numeric and time formatting follows the active locale; frequency and sample
  rate values keep their conventional units.
- The GUI default is `Yu Gothic UI` on Windows, `Hiragino Sans` on macOS, and
  `Noto Sans CJK JP` on Linux. These families cover Japanese UI text without an
  embedded CJK font; `cosmic-text` retains its system-font fallback when the
  preferred family is unavailable.

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
decoded callsigns, and overrun counts all come from the worker. When no
reception is in progress, the canvas shows a blank raster sized to the selected
mode.

A newly decoded FSKID takes the QSO panel's contact callsign field, which in
turn recomposites the transmit preview through the `contact.callsign` template
variable. The identifier names the station on the air, so it replaces whatever
the field held rather than deferring to it. Only an arrival writes: the worker
republishes every identifier it has decoded on each snapshot, so the interface
follows the count of identifiers it has already adopted and leaves the field
editable between receptions. That count is followed downwards as well, because
reopening a device restarts the worker with an empty list.

Transmit is implemented end to end. Template and stock changes enqueue a
latest-wins composite request. Set for transmit freezes the completed preview,
and TX opens the selected output device, primes a bounded queue, and starts
playback. Progress is reported by the row being transmitted: the worker
publishes the sample window the image raster occupies within the transmission,
and the interface maps the samples consumed by the device callback onto a row
within that window. The callback is the clock rather than the worker's
generated count, because the worker runs ahead to keep the queue full. The
leader and the station identifier fall outside the window and are named in the
badge and status bar instead of being reported as a row. Playback
underrun is reported as a transmission error, and Stop TX closes playback and
cancels the worker. TX remains actionable while prerequisites are missing; its
hover text and the status bar report the missing frame, output device, or valid
station callsign instead of silently disabling the button.

The mode dropdowns are already driven by `ModeSpec` support, so they list
exactly the modes the core can encode or decode.

## Prerequisites

Automatic history retains completed receptions and interrupted receptions that
reached at least 65 percent. The application does not browse them itself: the
files sit in the operator's own pictures directory, where the file manager
already shows them with thumbnails, sorting, and everything else a browser of
its own would have to reimplement. The File menu opens that directory, and the
receive controls that act on stored entries remain to be implemented.

The application storage directories and an empty default configuration file
are initialized at startup as described in [architecture.md](architecture.md).
Template editing, PTT, CAT, and logging remain planned gaps.

## Verification Strategy

Core behavior stays covered by the existing deterministic tests. The interface
adds:

- State-model tests drive actions without constructing a window, including tab
  switching during an active receive session and queue-overflow reporting.
- Worker tests that feed recorded PCM through the receive worker and assert the
  published snapshot sequence, reusing the synthesized signals already used by
  the offline integrations.
- A locale test asserting that every message key present in the default locale
  exists in each additional locale.
