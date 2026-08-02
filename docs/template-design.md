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
| `image` | PNG asset reference | `position`, `size` | None |
| `rximage` | None | `position`, `size` | None |
| `text` | Interpolated text | `position`, `font`, `fill` | `stroke` |
| `rect` | None | `position`, `size`, at least one paint | `fill`, `stroke` |
| `ellipse` | None | `position`, `size`, at least one paint | `fill`, `stroke` |
| `line` | None | `start`, `end`, `stroke` | None |
| `group` | None | At least one layer | `position` |

The supported child properties are:

```kdl
position x=(fw)0 y=(fh)0 anchor="top-left"
size width=(fw)10 height=(fh)10 fit="contain"
start x=(fw)0 y=(fh)0
end x=(fw)100 y=(fh)100
font family="Noto Sans" size=(fh)9 weight=700
fill color="#ffffff"
stroke color="#182030" width=(em)0.08
```

`position.anchor` defaults to `top-left`. Image `fit` defaults to `contain` and
accepts `contain`, `cover`, `stretch`, or `preserve`. `aspect="preserve"` is
accepted as an equivalent spelling, but `fit` and `aspect` cannot both appear.
Shapes require both width and height. Images may omit one dimension, in which
case it is derived from the PNG aspect ratio.

Colors use `#RRGGBB` or `#RRGGBBAA`. Layer nodes, child nodes, and properties
outside this schema are errors. Duplicate properties and duplicate singleton
children are also errors. Dimensions must be finite and non-negative;
layer width and height values that resolve to zero are rejected before
rasterization.

Groups add their optional `position` to every nested layer. Child coordinates
remain frame-relative rather than becoming percentages of a group bounding box.
The initial implementation does not provide rotation, arbitrary transforms,
clipping, shadows, gradients, rounded rectangles, or multiline text.

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

## Variables

Interpolation uses explicit names enclosed in `${...}`:

```kdl
text "To ${contact.callsign} from ${station.callsign}"
```

Variables belong to named domains rather than a flat table of single-character
macros. Anticipated values include:

- `station.callsign`, `station.name`, and `station.qth`
- `contact.callsign`, `contact.name`, and `contact.qth`
- `report.sent` and `report.received`
- `radio.frequency` and `radio.band`
- `tx.timestamp` and `rx.timestamp`
- `application.version`

The evaluation context supplies typed values, including the image used by an
`rximage` layer. Text interpolation converts only values used in text; images
and other resources should be referenced as typed properties rather than
converted through strings.

The initial implementation accepts text, signed integer, floating-point, and
boolean variable values. It applies no formatting expressions; callers provide
preformatted text for values such as dates and times. A missing variable is a
render error. Variable names consist of dot-separated ASCII identifier segments.
`$${name}` produces the literal text `${name}`.

## Text Rendering

Text style is independent of the platform font API. A text layer can specify a
font family, relative size, weight, fill, alignment, and effects.

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
renderer never opens a path from the template directly. The initial
implementation accepts PNG-encoded assets only and passes them to `usvg` through
private virtual resource URIs.

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
