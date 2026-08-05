# Template Design

This document defines the intended design of the RSSSTV transmit image template
system. Templates are portable RGBA overlays that are rendered independently of
the selected SSTV mode, source image, UI toolkit, and transmit encoder.

The initial implementation is provided by the `rssstv-template` crate. It parses
KDL v2, generates a controlled static SVG subset, and rasterizes that SVG with
`usvg` and `resvg`.

## Goals

- Use an existing human-readable data definition language rather than inventing
  a general-purpose or executable language.
- Let one template adapt to different SSTV frame dimensions.
- Use explicit, named variables instead of MMSSTV percent parameters such as
  `%m` and `%c`.
- Keep templates independent of the background image and its source.
- Use alpha compositing throughout the template pipeline.
- Keep the scene representation suitable for both hand editing and a graphical
  template editor.

## Non-Goals

- Executing arbitrary code from a template.
- Reproducing the MMSSTV MTM/MTI binary format.
- Reproducing Windows GDI font rasterization exactly.
- Supporting color-key transparency. Transparent image assets should carry an
  alpha channel, such as an RGBA PNG.
- Encoding the target SSTV mode or fixed pixel dimensions in a template.

## File Format

Templates use [KDL](https://kdl.dev/) as their data definition language. A file
does not have a wrapper node or a required format-version declaration. Its
top-level nodes are an ordered list of layers. Nodes later in the document are
drawn over earlier nodes.

```kdl
image "assets/logo.png" {
    position x=(fw)95 y=(fh)5 anchor="top-right"
    size width=(fw)14 aspect="preserve"
}

rximage {
    position x=(fw)5 y=(fh)22
    size width=(fw)35 height=(fh)45 fit="cover"
}

text "To ${contact.callsign}" {
    position x=(fw)50 y=(fh)8 anchor="top-center"
    font family="Noto Sans" size=(fh)9 weight=700
    fill color="#ffffff"
    stroke color="#182030" width=(em)0.08
}

rect {
    position x=(fw)5 y=(fh)78
    size width=(fw)90 height=(fh)17
    fill color="#101820cc"
}
```

The initial format should evolve through additive nodes and properties. A
versioning mechanism should only be introduced if a concrete incompatible
change requires one.

## Scene Model

The KDL document describes a scene graph whose root is implicit. The initial
layer types are:

- `image`
- `rximage`
- `text`
- `rect`
- `ellipse`
- `line`
- `group`

Geometry, paint, text style, effects, and asset references remain separate in
the scene model even when KDL expresses them compactly.

Groups provide nested positioning and compositing without changing the root
document structure. Layer order is stable and follows document order at every
level.

### Initial Schema

The initial implementation supports these layer forms:

| Layer | Argument | Required children | Optional children |
| --- | --- | --- | --- |
| `image` | Image asset reference (PNG, JPEG, BMP, or WebP) | `position`, `size` | `clip` |
| `rximage` | None | `position`, `size` | `clip` |
| `text` | Interpolated text | `position`, `font`, `fill` | `stroke` |
| `rect` | None | `position`, `size`, at least one paint | `fill`, `stroke` |
| `ellipse` | None | `position`, `size`, at least one paint | `fill`, `stroke` |
| `line` | None | `start`, `end`, `stroke` | None |
| `group` | None | At least one layer | `position` |

The supported child properties are:

```kdl
position x=(fw)0 y=(fh)0 anchor="top-left" rotate=-12
size width=(fw)10 height=(fh)10 fit="contain" radius=(fh)2
start x=(fw)0 y=(fh)0
end x=(fw)100 y=(fh)100
font family="Noto Sans" size=(fh)9 weight=700 style="italic" leading=1.2
fill color="#ffffff"
stroke color="#182030" width=(em)0.08
clip shape="circle"
```

A `fill` states either one color or a gradient, described below. A `stroke` is
always one color.

`position.anchor` defaults to `top-left`. Image `fit` defaults to `contain` and
accepts `contain`, `cover`, `stretch`, or `preserve`. `aspect="preserve"` is
accepted as an equivalent spelling, but `fit` and `aspect` cannot both appear.
Shapes require both width and height. Images may omit one dimension, in which
case it is derived from the PNG aspect ratio.

`font.style` is optional, defaults to `normal`, and accepts `normal` or
`italic`.

`position.rotate`, `size.radius`, `font.leading`, and `clip` are described in
the sections below. A line endpoint takes coordinates alone, so `rotate` is an
error on `start` and `end`.

Colors use `#RRGGBB` or `#RRGGBBAA`. Layer nodes, child nodes, and properties
outside this schema are errors. Duplicate properties and duplicate singleton
children are also errors. Dimensions must be finite and non-negative;
layer width and height values that resolve to zero are rejected before
rasterization.

Groups add their optional `position` to every nested layer. Child coordinates
remain frame-relative rather than becoming percentages of a group bounding box.
The initial implementation does not provide arbitrary transforms, shadows, or
pattern fills.

### Rotation

Any `position` that anchors a layer, and a group's own `position`, accepts a
`rotate` in degrees:

```kdl
text "CQ SSTV" {
    position x=(fw)50 y=(fh)55 anchor="center" rotate=-12
    font family="Noto Sans" size=(fh)12 weight=700
    fill color="#ffffff"
}
```

The angle is measured clockwise, the same convention a linear gradient uses,
and defaults to 0. A layer turns about the point its `position` names rather
than about its own center, so the anchor keeps meaning what it meant: a layer
anchored `bottom-right` against a frame corner stays pinned to that corner as
it turns. A rotated group turns its whole subtree about the group's own point.

Rotation is stated on the point it turns about rather than as a separate child
node, because there is nothing else for a rotation to be about, and because a
group then gets one without a second spelling.

A gradient fill resolves against the layer's box before the layer turns, so the
gradient turns with what it paints.

### Rounded Corners and Clipping

A `size` on a `rect`, an `image`, or an `rximage` accepts a `radius`, which
rounds the corners of that layer's box:

```kdl
rect {
    position x=(fw)5 y=(fh)78
    size width=(fw)90 height=(fh)17 radius=(fh)2
    fill color="#101820cc"
}
```

An `ellipse` has no corner to round, so `radius` is an error on one.

An image layer also accepts a `clip`, which cuts it to a shape that is not a
box:

```kdl
rximage {
    position x=(fw)5 y=(fh)22
    size width=(fw)35 height=(fh)35 fit="cover"
    clip shape="circle"
}
```

`clip.shape` accepts `circle`, which is the largest circle centered in the
layer box, and `ellipse`, which is the ellipse inscribed in it. A layer cannot
both round its corners and be clipped, since the clip decides the outline.

Rounded corners and clipping are separate spellings because they are separate
things. A shape's rounding belongs to the outline that its `fill` and `stroke`
follow, so a stroke turns the corner. A clip only cuts, so a stroked shape
would come out sliced in half rather than followed. Images carry neither fill
nor stroke, so for them the two produce the same picture and `radius` is
implemented as a clip.

### Wrapped Text

A newline in a text layer starts a new line. Nothing else about the layer
changes; the lines share one position, one anchor, one font, and one fill:

```kdl
text "CQ CQ CQ\nde ${station.callsign}\npse K" {
    position x=(fw)5 y=(fh)10 anchor="top-left"
    font family="Noto Sans" size=(fh)8 weight=700 leading=1.4
    fill color="#ffffff"
}
```

`font.leading` is the multiple of the font size between one baseline and the
next, and defaults to 1.2. The anchor applies to the block rather than to its
first line, so a `bottom-*` anchor holds the last line where a single line
would have sat, and `center` centers the block.

A newline that arrives through `${...}` wraps the same way a newline written in
the template does, since the text is split after interpolation.

A gradient fill spans the whole block rather than restarting on each line,
because the lines are one text layer and the gradient resolves against its box.

### Gradient Fills

A `fill` on a `rect`, an `ellipse`, or a `text` states one gradient instead of
one color, as a `gradient` kind and a list of stops:

```kdl
rect {
    position x=(fw)0 y=(fh)0
    size width=(fw)100 height=(fh)6
    fill gradient="linear" angle=0 {
        stop offset=0 color="#00ffff"
        stop offset=1 color="#00ff00"
    }
}
```

`gradient` accepts `linear` and `radial`. A gradient requires at least two
`stop` nodes; each carries an `offset` between 0 and 1 and a color, and the
offsets must not decrease. A stop color may carry alpha, so a gradient can fade
a layer out rather than only recolor it. `color` and `gradient` cannot both
appear on one `fill`.

A linear gradient's optional `angle` is in degrees, measured clockwise from the
frame's positive x axis: 0 runs left to right, 90 runs top to bottom, and the
default is 0. A radial gradient starts at the center of the layer and reaches
its last stop at half the layer's width and height, so `angle` is an error on
one.

A gradient is laid out over the layer's own bounding box rather than over the
frame, which is what lets a text gradient follow the text it paints without the
template measuring glyphs. The box is normalized to a unit square first, so an
angle other than a multiple of 90 degrees is sheared by the box's aspect ratio,
in the same way an SVG `objectBoundingBox` gradient is. A `text` gradient spans
the whole run of text, not each glyph.

Strokes take one color. A gradient outline would have to resolve against the
same box as the fill it surrounds, and nothing in the ported templates or the
format's own use asks for one.

## Coordinate System

The renderer receives the target frame dimensions from its caller. Templates do
not declare a canvas size.

KDL type annotations identify relative units without quoting values:

| Unit | Meaning | Example |
| --- | --- | --- |
| `(fw)` | Percentage of frame width | `x=(fw)95` |
| `(fh)` | Percentage of frame height | `y=(fh)5` |
| `(em)` | Multiple of the current font size | `width=(em)0.08` |

Thus `(fw)100` is the full frame width and `(fh)100` is the full frame height.
Coordinates remain floating-point values until rasterization.
Position, line endpoint, and group-offset values may be negative, allowing a
layer to extend beyond the top or left frame edge. Sizes, font sizes, and stroke
widths remain non-negative.

Anchors define which point of an object is placed at its position. Expected
anchors include `top-left`, `top-center`, `top-right`, `center`, `bottom-left`,
`bottom-center`, and `bottom-right`. Anchoring avoids encoding mode-specific
pixel offsets and allows edge-aligned content to remain stable across different
aspect ratios.

Image sizing supports `contain`, `cover`, `stretch`, and explicitly sized
aspect-preserving behavior.

The initial renderer accepts `(em)` where a current text font size exists, such
as text stroke width. Using `(em)` for non-text geometry is a render error.

### Received Image Layer

An `rximage` layer displays the final received image supplied by the caller:

```kdl
rximage {
    position x=(fw)5 y=(fh)22
    size width=(fw)35 height=(fh)45 fit="cover"
}
```

The layer specifies only layout and rendering behavior. The caller is
responsible for identifying and providing the appropriate final received image
as typed input to the renderer. The template renderer must not implicitly read
receive history, application state, or files to resolve `rximage`.

If a template contains `rximage` and the caller does not provide an image,
rendering fails. The renderer encodes the supplied RGB image as an in-memory PNG
for the SVG backend; it does not persist that encoding or add it to the template.

The application supplies the last reception it kept: one that completed, or one
that was interrupted with at least 65 percent of its rows decoded, which is the
same rule the received folder uses. The image is held in memory, so the layer
follows the last reception whether or not receptions are being written to disk.
Until a reception qualifies, the layer shows the mode's test pattern, so a
template built around a reception composes from the first launch rather than
failing to render.

## Variables

Interpolation uses explicit names enclosed in `${...}`:

```kdl
text "To ${contact.callsign} from ${station.callsign}"
```

Variables belong to named domains rather than a flat table of single-character
macros. Anticipated values include:

- `station.callsign`, `station.qth`, `station.grid`, and `station.name`
- `contact.callsign`
- `report.sent`, `report.number`, and `report.received`
- `radio.frequency` and `radio.band`
- `tx.timestamp.utc`, `tx.timestamp.local`, `rx.timestamp.utc`, and
  `rx.timestamp.local`
- `custom.*`, whose names the operator chooses
- `application.version`

Contact detail beyond the callsign is deliberately absent. RSSSTV does not set
out to keep a QSO log, so a field that exists only to be typed into a template
is not worth the entry it would need.

The evaluation context supplies typed values, including the image used by an
`rximage` layer. Text interpolation converts only values used in text; images
and other resources should be referenced as typed properties rather than
converted through strings.

The implementation accepts text, signed integer, floating-point, boolean, and
timestamp variable values. A missing variable is a render error. Variable names
consist of dot-separated ASCII identifier segments. `$${name}` produces the
literal text `${name}`.

### Timestamp Formats

A timestamp variable carries an instant in a named time zone rather than
preformatted text, and a text expression says how to write it:

```kdl
text "${tx.timestamp.utc:%d %b %Y %H:%MZ}"
```

The format follows the first colon and runs to the closing brace, so a format
containing colons needs no escaping. It is a `jiff` `strtime` format string,
the same `%`-directive vocabulary as `strftime`; `jiff` is already the
application's date library, so templates and the rest of RSSSTV describe time
the same way. A directive the formatter rejects is a render error, reported
like any other template error. A timestamp written without a format uses
`%Y-%m-%d %H:%M`, and a format applied to any other kind of value is an error.

Each timestamp is supplied in two zones rather than being converted by the
template: `.utc` for the on-air convention and `.local` for the operator's own
clock.

Because such a template stops being true as the clock moves,
`Template::uses_timestamps` reports whether any text reads a timestamp out of a
given variable set. A caller that holds a rendered overlay uses it to decide
whether that overlay has to be rendered again; the finest unit the default
format shows is a minute, so refreshing as the minute changes is enough.

The desktop composition worker currently supplies:

| Variable | Source |
| --- | --- |
| `station.callsign`, `station.qth`, `station.grid` | Station dialog |
| `contact.callsign` | QSO call field |
| `report.sent`, `report.number` | QSO RSV and serial-number fields |
| `report.received` | QSO received-RSV field |
| `radio.frequency`, `radio.band` | Fixed until rig control arrives |
| `tx.timestamp.*` | The clock, as the composition is made |
| `rx.timestamp.*` | When the image `rximage` shows was adopted |
| `custom.*` | The template variable dialog |
| `application.version` | The crate version |

`radio.frequency` and `radio.band` are a fixed pair rather than an absent one:
a template that prints the frequency has to compose to something before there
is a radio to ask, and a missing variable would refuse to render at all. Rig
control replaces the values without the template changing.

`rx.timestamp.*` follows the same rule as the `rximage` layer it describes: the
test pattern counts as adopted at startup, so the variable resolves from the
first launch rather than failing until a reception arrives.

Image assets are resolved relative to the template first, then from the
application's shared `assets` directory; an `assets/` prefix is stripped for
the shared lookup.

## Ported MMSSTV Templates

`templates/` holds the five templates MMSSTV ships as `def1.mtm` through
`def5.mtm`, ported to this format. They are not installed anywhere; copy the
ones that are wanted into the application's templates directory. Each file
records in a comment what its original did that this format cannot.

The ports follow one set of rules:

- Geometry becomes frame-relative. MMSSTV draws templates at 320 by 256 and
  scales the result, so every stored pixel is a percentage of that.
- Each layer is anchored to the frame corner it sits nearest, so a template
  built for a 4:3 mode keeps its layout in every other one. MMSSTV's own
  `m_RightAdj` already says which horizontal edge an item was placed by.
- A right or bottom edge is taken from the text's own extent rather than from
  the stored rectangle, which includes an allowance for effects. An edge that
  falls outside the frame is placed on it.
- A gradient fill becomes a gradient. MMSSTV's two-color fill becomes two stops
  at 0 and 1; its four-color fill becomes four stops at 0, 1/3, 2/3, and 1,
  which is where its three equal segments meet. Its vertical flag becomes an
  `angle` of 90.
- An outline becomes a text stroke. A drop shadow, an extrusion, and an emboss
  have no equivalent, so each keeps the outline it was drawn with, or is
  approximated by one in the shadow color when it had none.
- Text cut out of the overlay in the transparent color becomes a transparent
  fill behind an opaque stroke. Alpha compositing gives the same reading as
  color keying without the color key.
- `%m`, `%c`, and `%r` become `${station.callsign}`, `${contact.callsign}`, and
  `${report.sent}`. MMSSTV's `%r` is the report being given to the other
  station, which is the one this application calls sent.
- A font that Windows does not ship, or that a font database reports under
  another family and weight, is named as the database has it.

## Text Rendering

Text style is independent of the platform font API. A text layer can specify a
font family, relative size, weight, normal or italic style, leading, fill,
alignment, and effects.

Outlining is a text effect rather than part of the font identity:

```kdl
text "${station.callsign}" {
    font family="Noto Sans" size=(fh)9 weight=700
    fill color="#ffffff"
    stroke color="#182030" width=(em)0.08
}
```

Using `(em)` for stroke and shadow dimensions keeps effects proportional to the
text as the output resolution changes. A renderer may implement an outline by
stroking glyph paths or expanding a glyph mask, but should document the visual
and metric guarantees it provides. Exact output may differ between font
backends when the selected font is unavailable or rasterized differently.

The renderer begins with an empty font database. Callers register in-memory
font data or explicitly request system-font discovery. A requested family that
is not present in the database is a render error. The generated SVG uses
`paint-order="stroke fill"` and a round stroke join so a wide outline is painted
behind the glyph fill.

## Rendering and Composition

A template renderer produces an RGBA image with the dimensions requested by its
caller. Transparent pixels are valid and expected; a template is not required
to cover the frame.

The public image representation uses straight alpha. `resvg` renders internally
to premultiplied RGBA; `rssstv-template` converts that buffer before returning
it. Fully transparent pixels are normalized to transparent black.

The background image is selected and prepared separately. Resizing, cropping,
and choosing whether to contain, cover, or stretch that image are application
responsibilities rather than template responsibilities.

```text
Background source
  -> resize/crop to the SSTV mode dimensions
  -> RGB background frame

Template and evaluation context
  -> render at the same dimensions
  -> RGBA overlay

RGB background frame + RGBA overlay
  -> alpha composite
  -> RgbImage
  -> TxEncoder
```

Composition uses source-over alpha blending directly on eight-bit sRGB channel
values, with integer rounding to the nearest output value. The background and
overlay must have identical dimensions. There is no sampled transparent color,
color-key import mode, or implicit conversion of a particular RGB value to
transparency.

`image` references are resolved through a caller-provided asset interface. The
renderer never opens a path from the template directly. The implementation
accepts PNG, JPEG, and WebP assets, passing their bytes to `usvg` through
private virtual resource URIs, and accepts BMP assets by decoding them and
re-encoding as PNG before the same step.

This pipeline keeps template parsing, variable evaluation, font and asset
rendering, source-image preparation, and SSTV encoding as separate concerns.
The `TxEncoder` continues to receive an already composed, mode-sized
`RgbImage`.

## Editor Implications

A graphical editor manipulates the same scene model represented by KDL. It can
preview the RGBA overlay over a checkerboard, a chosen test image, or the current
transmit image without storing that background in the template.

The editor should preserve document order because it defines the layer order.
Frame-relative geometry should be the normal editing mode so switching between
PD120, Scottie 2, and other modes does not create separate templates solely due
to resolution differences.

## Composition Example

The `rssstv-template` crate includes a command-line library example that renders
a KDL template at the background image dimensions and saves the composed RGB
image:

```text
cargo run -p rssstv-template --example compose -- template.kdl background.png output.png
```

`rssstv-template/examples/sample.kdl` is a variable-free sample suitable for a
quick render.

The example resolves `image` references relative to the process current working
directory, explicitly loads system fonts, and supplies an opaque white image for
every `rximage` layer. It supplies no text variables, so a template containing a
`${...}` expression fails with the normal missing-variable error.

The `encode-wav` integration instead renders at the selected SSTV mode's
transport dimensions:

```text
cargo run -p encode-wav -- --callsign N0CALL template.kdl background.png robot36 output.wav
```

It cover-resizes and center-crops the background, supplies that prepared image
to every `rximage` layer, defines `${station.callsign}` from the normalized
callsign and `${tx.timestamp.utc}` and `${tx.timestamp.local}` from the clock,
and resolves template image assets relative to the template file. It supplies
nothing else, so a template written for the desktop application may name a
variable this command refuses.
