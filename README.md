# RSSSTV

Rust SSTV protocol, DSP, template-rendering, and WAV integration components.

Development documentation is in [docs/memo/](docs/memo/README.md), divided into
the SSTV protocols themselves, the behavior of the original MMSSTV, and this
project. [docs/help/](docs/help/index.md) is the operator's manual the release
archives carry, rendered to HTML by `docs/help/build.sh`.

## Application

`rssstv` is the desktop interface, built with egui and eframe:

```text
cargo run -p rssstv
```

Selecting an input device opens a capture stream and starts a worker that
demodulates the audio, detects the mode from VIS, and decodes the image
progressively. Mode, decoded rows, input level, synchronization strength, and
decoded FSKID callsigns come from that worker.

To transmit, select an output device from Settings, enter My call, select a KDL
template and stock image, and choose Set for transmit after the composite
preview is ready. TX streams the complete VOX, VIS, raster, footer, FSKID, and
trailing-silence sequence to the selected device. The same button stops an
active transmission. See
[docs/memo/rssstv/gui-design.md](docs/memo/rssstv/gui-design.md) for the design
and remaining work.

Rig control goes through Hamlib's `rigctld` rather than a linked library, so
there is nothing to build and the serial port stays available to the logger.
Start `rigctld` for your rig, connect from the Radio panel, and transmissions
key it and read its frequency into `${radio.frequency}` and `${radio.band}`.
The same panel changes band and steps up and down it.

What is actually sent is decided by a Lua script, because a station keys its
rig in more ways than one protocol covers. The band plan the radio panel offers
comes from a file beside it. Both are built in and need no files; write either
out from Settings › Rig Control, as `rigcontrol.lua` and `bands.toml` beside
`config.toml`, to take it over. See
[docs/memo/rssstv/rig-control.md](docs/memo/rssstv/rig-control.md).

On Linux the window icon comes from a desktop entry rather than from the
application, because a Wayland compositor has no other way to learn one. The
application names itself `rssstv`, and the compositor looks for the entry of
the same name; installing it and the icon it points at is what makes the icon
appear in the task switcher and the dock:

```text
install -Dm644 rssstv/assets/rssstv.desktop \
  ~/.local/share/applications/rssstv.desktop
install -Dm644 rssstv/assets/icon.png \
  ~/.local/share/icons/hicolor/512x512/apps/rssstv.png
update-desktop-database ~/.local/share/applications
```

The entry's `Exec=rssstv` expects the executable on `PATH`, which
`cargo install --path rssstv` arranges; point it at the build directory
instead if you are running from `cargo run`.

[templates/](templates) holds the five templates MMSSTV ships, ported to the
KDL format. Copy the ones you want into the application's templates directory;
each file records in a comment what its original did that this format cannot.

## Encode WAV

`encode-wav` renders a KDL template over a background image and writes a
complete SSTV transmission as streaming 48 kHz mono 16-bit PCM:

```text
cargo run -p encode-wav -- [--callsign CALLSIGN] <TEMPLATE.kdl> <BACKGROUND_IMAGE> <MODE> <OUTPUT.wav>
```

The callsign defaults to `N0CALL`, is uppercased, replaces `${station.callsign}`
in the template, and is sent as the trailing FSKID. `${tx.timestamp.utc}` and
`${tx.timestamp.local}` are set from the clock. The prepared background is also
available to `rximage` layers. Backgrounds are resized to cover the selected
mode and center-cropped. Template image assets are resolved relative to the
template file.

Supported transmit modes are Robot 36/72, Scottie 1/2/DX, Martin 1/2, and
PD50/90/120/160/180/240/290. Mode arguments ignore ASCII case, spaces, hyphens,
and underscores.

## Decode WAV

```text
cargo run -p decode-wav -- [--packet-size SAMPLES] <INPUT.wav> <OUTPUT_IMAGE>
```
