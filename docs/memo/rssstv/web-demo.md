# The Browser Receive Demo

`web-demo` compiles the receive path to WebAssembly and drives it from a page
that takes an audio file or a microphone. It exists because the portability of
the core is otherwise only an assertion: the crates build without `std`, and CI
proves that they do, but nothing demonstrates that the same code decodes a
picture somewhere other than a desktop.

## Why wrapping is all it takes

The receive path — `rssstv-dsp`, `rssstv-fskid`, `rssstv-sstv`, and
`rssstv-demodulator` — starts no threads, reads no clock, opens no file, and
uses neither channels nor randomness. Nothing in it reaches a facility
`wasm32-unknown-unknown` lacks, so the crate adds no signal processing of its
own and changes nothing in the core. It is a `wasm-bindgen` surface over
[`ReceivePipeline`](../../../rssstv-demodulator/src/pipeline.rs), which is the
same wiring `decode-wav` uses.

`rssstv-demodulator` is not itself `no_std`, and it does not need to be. Its
uses of the standard library are `VecDeque`, `core::f64` constants reached
through `std`, `Reverse`, and the inherent float methods; all of those exist on
wasm, where `std` is available. Making it `no_std` would be a separate change
answering to embedded targets, not to this one.

## The exported surface

One object, fed and polled:

- `new SstvReceiver(sampleRateHz, liveSlant, stagingSeconds)` builds the
  pipeline. Nothing resamples, so the rate must be the rate of the source, and
  anything below 6 kHz is refused by `Demodulator::new`.
- `push(samples)` takes normalized mono `f32` and copies it into linear memory.
  The copy is one `memcpy` against a Hilbert transform and a filter bank per
  sample, so it is not worth avoiding.
- `status()` returns a snapshot: mode name, state, completed rows, image size,
  image revision, AFC offset, fitted raster rate, callsigns, and input level.
  It is a `wasm-bindgen` object and the caller frees it.
- `image_rgba()` widens the decoder image to RGBA for `ImageData`.
- `drain_log()` takes the event lines recorded since the last call.
- `finish()` closes the stream, which is also what applies the whole-reception
  slant refit, and `reset()` starts another.

Events are drained rather than delivered through a callback. A callback per
event crosses the boundary once a row for no gain, and what a page displays —
how far decoding has got — is a counter in the state rather than a stream of
notifications. `RxEvent::RowDecoded` is therefore recorded nowhere; the other
four events become log lines because they say something a row count does not.

The image is copied rather than exposed as a pointer. A `Uint8ClampedArray`
over linear memory would avoid the copy but is detached by any allocation that
grows the memory, which makes it correct only for as long as nothing awaits
between taking the view and painting it. The copy is a few hundred kilobytes at
a rate of a few rows a second, and the crate keeps the `forbid(unsafe_code)` the
rest of the workspace has.

Two things the page cannot show, because `ReceivePipeline` consumes them:
`DemodulatedChunk::sync_strength` never leaves the pipeline, so the level meter
shows the peak amplitude of the pushed packet instead, and station identifiers
are only returned by `finish`, so callsigns appear when a reception ends rather
than as they decode. Neither is worth widening the core's surface for.

## `decodeAudioData` does not return the file's samples

Audio files are parsed by the page rather than handed to the Web Audio API.
`BaseAudioContext.decodeAudioData` resamples to the context's rate, which alone
would be reason enough to read the header and build the context around it, but
it also does not reproduce the file's samples when no resampling is involved:
decoding a 48 kHz WAV into a 48 kHz context returns about half of its samples
off by a fraction of a least significant bit. That is inaudible and very nearly
invisible — it moved 27 of a Robot 36 picture's 76,800 pixels, by at most 2 of
255, all of them on hard chroma edges — but it means the demo cannot be checked
against `decode-wav` for equality, which is the only cheap way to know the
decode is right.

Reading the chunks directly is some forty lines, matches `decode-wav`'s scaling
exactly, accepts the rates below 8 kHz that the API will not open a context at,
and makes the pictures identical. `decodeAudioData` remains the fallback for
formats that are not PCM WAV, where the page says so in its log.

## Staging and memory

Staged samples cost three bytes each, and the file path states the recording's
length, as `decode-wav` does. The bound is not an allocation: `RxDecoder`
reserves `max_samples` capped at twice the detected mode's raster, so a
generous figure costs nothing until a long mode is actually detected.

The microphone states 360 seconds, which covers the longest decodable mode, and
that bound is why the page has to end receptions rather than leave the pipeline
running. `ReceivePipeline` configures `auto_stop: false`, and every sample after
the last row goes to `stage_for_refinement`, so a microphone left open after a
picture finishes eventually returns `StagingCapacityExceeded`. The page waits
fifteen seconds past completion — which is also the tail the refit is fitted
against, and long enough for the FSKID that follows the picture — then calls
`finish` and starts over. It applies the same treatment to a reception that
stops producing rows for twenty seconds.

## Microphone audio must not be processed

The capture constraints turn off echo cancellation, noise suppression, and
automatic gain control, and none of the three is optional. Noise suppression is
tuned for speech and treats a steady tone as stationary noise, which attenuates
the signal being demodulated. Automatic gain control rescales continuously,
and while the demodulator does not care about amplitude, the sync detector
scores a normalized envelope that gain pumping reshapes. Echo cancellation
adapts against a far end that does not exist and mangles the waveform. The
browser cannot reach the processing the operating system applies to the device,
so the page also says to turn off Windows audio enhancements and macOS voice
isolation.

Capture is an `AudioWorkletProcessor` that gathers 128-frame quanta into 2048
samples and posts them; the decoding happens on the main thread. Running the
decoder inside the worklet is possible but not sensible: the global scope has no
`fetch` and no streaming instantiation, so the module has to be compiled outside
and passed in, and the work is far heavier than a render quantum's budget, which
turns every hesitation into an audio-thread stall instead of a dropped frame.

## Building and serving

```text
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
wasm-pack build web-demo --target web --out-dir www/pkg --release
python -m http.server -d web-demo/www 8080
```

The `wasm-opt` that wasm-pack downloads predates the WebAssembly features rustc
now emits by default, so `web-demo/Cargo.toml` names them in
`[package.metadata.wasm-pack.profile.release]`; without that the size pass fails
validation on every build.

A server is required rather than convenient. `file://` blocks both the module
and the `.wasm` fetch, and `getUserMedia` needs a secure context, which means
HTTPS or localhost.

## What is verified

Pictures encoded by `encode-wav` and decoded by `decode-wav` are the reference.
Robot 36, Scottie 1, and PD120 at 48 kHz, and Robot 36 decimated to 16 kHz, all
decode in the browser to images identical to `decode-wav`'s, with the same
detected mode, AFC offset, fitted raster rate, row count, and FSKID. A 4 kHz
file is refused by the constructor, and audio with no VIS reports that nothing
was detected without disturbing the picture already on screen.
