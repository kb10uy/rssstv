# RSSSTV

Rust SSTV protocol, DSP, template-rendering, and WAV integration components.

Documentation is in [docs/](docs/README.md), divided into the SSTV protocols
themselves, the behavior of the original MMSSTV, and this project.

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
active transmission. See [docs/rssstv/gui-design.md](docs/rssstv/gui-design.md)
for the design and remaining work.

Rig control goes through Hamlib's `rigctld` rather than a linked library, so
there is nothing to build and the serial port stays available to the logger.
Start `rigctld` for your rig, switch Rig Control on, and transmissions key it
and read its frequency into `${radio.frequency}` and `${radio.band}`. What is
sent at each moment — keying, unkeying, connecting, or arriving on a band — is
written in `config.toml` and defaults to plain PTT. See
[docs/rssstv/rig-control.md](docs/rssstv/rig-control.md).

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
