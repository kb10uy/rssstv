# RSSSTV

Rust SSTV protocol, DSP, template-rendering, and WAV integration components.

## Application

`rssstv` is the desktop interface, built with iced:

```text
cargo run -p rssstv
```

It is currently an interface shell. Tabs, mode selection, DSP toggles, QSO
fields, locale switching, and the template and stock lists are interactive, but
there is no audio input or output yet, so it neither receives nor transmits.
Receive activity is simulated over a generated test pattern. See
[docs/gui-design.md](docs/gui-design.md) for the design and the remaining work.

## Encode WAV

`encode-wav` renders a KDL template over a background image and writes a
complete SSTV transmission as streaming 48 kHz mono 16-bit PCM:

```text
cargo run -p encode-wav -- [--callsign CALLSIGN] <TEMPLATE.kdl> <BACKGROUND_IMAGE> <MODE> <OUTPUT.wav>
```

The callsign defaults to `N0CALL`, is uppercased, replaces `${mycall}` in the
template, and is sent as the trailing FSKID. The prepared background is also
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
