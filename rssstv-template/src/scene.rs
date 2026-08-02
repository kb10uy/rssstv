use std::collections::BTreeMap;
use std::fmt;

/// A parsed template whose layers retain document order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template {
    pub(crate) layers: Vec<Layer>,
}

impl Template {
    /// Returns the top-level layers in back-to-front order.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }
}

/// A frame-relative or font-relative length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// Percentage of frame width.
    FrameWidth(f64),
    /// Percentage of frame height.
    FrameHeight(f64),
    /// Multiple of the current font size.
    Em(f64),
}

/// The point of a layer placed at its position.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Anchor {
    /// Top-left corner.
    #[default]
    TopLeft,
    /// Center of the top edge.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Geometric center.
    Center,
    /// Bottom-left corner.
    BottomLeft,
    /// Center of the bottom edge.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

/// Image fitting behavior within a specified rectangle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageFit {
    /// Fit entirely within the rectangle while preserving aspect ratio.
    #[default]
    Contain,
    /// Cover the rectangle while preserving aspect ratio and cropping overflow.
    Cover,
    /// Stretch independently in both axes.
    Stretch,
    /// Preserve aspect ratio when only one dimension is specified.
    Preserve,
}

/// An eight-bit RGBA color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Position {
    pub x: Length,
    pub y: Length,
    pub anchor: Anchor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayerSize {
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub fit: ImageFit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stroke {
    pub color: Color,
    pub width: Length,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FontStyle {
    #[default]
    Normal,
    Italic,
}

impl FontStyle {
    pub const fn as_svg(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Italic => "italic",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Font {
    pub family: String,
    pub size: Length,
    pub weight: u16,
    pub style: FontStyle,
}

/// A template layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Layer {
    /// A caller-resolved PNG asset.
    Image(ImageLayer),
    /// The caller-provided final received image.
    ReceivedImage(ReceivedImageLayer),
    /// A single line of interpolated text.
    Text(TextLayer),
    /// A rectangle.
    Rectangle(RectangleLayer),
    /// An ellipse.
    Ellipse(EllipseLayer),
    /// A line segment.
    Line(LineLayer),
    /// A translated nested layer sequence.
    Group(GroupLayer),
}

/// A referenced image layer.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageLayer {
    pub(crate) reference: String,
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
}

/// A final received-image layer.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedImageLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
}

/// An interpolated text layer.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayer {
    pub(crate) text: String,
    pub(crate) position: Position,
    pub(crate) font: Font,
    pub(crate) fill: Color,
    pub(crate) stroke: Option<Stroke>,
}

/// A rectangle layer.
#[derive(Clone, Debug, PartialEq)]
pub struct RectangleLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) fill: Option<Color>,
    pub(crate) stroke: Option<Stroke>,
}

/// An ellipse layer.
#[derive(Clone, Debug, PartialEq)]
pub struct EllipseLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) fill: Option<Color>,
    pub(crate) stroke: Option<Stroke>,
}

/// A line-segment layer.
#[derive(Clone, Debug, PartialEq)]
pub struct LineLayer {
    pub(crate) start: Position,
    pub(crate) end: Position,
    pub(crate) stroke: Stroke,
}

/// A translated nested layer sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupLayer {
    pub(crate) position: Option<Position>,
    pub(crate) layers: Vec<Layer>,
}

/// A typed value available to text interpolation.
#[derive(Clone, Debug, PartialEq)]
pub enum VariableValue {
    /// Text copied without additional formatting.
    Text(String),
    /// A signed decimal integer.
    Integer(i64),
    /// A finite decimal number.
    Decimal(f64),
    /// A boolean rendered as `true` or `false`.
    Boolean(bool),
}

impl fmt::Display for VariableValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => formatter.write_str(value),
            Self::Integer(value) => value.fmt(formatter),
            Self::Decimal(value) => value.fmt(formatter),
            Self::Boolean(value) => value.fmt(formatter),
        }
    }
}

/// Named values supplied for text interpolation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variables {
    values: BTreeMap<String, VariableValue>,
}

impl Variables {
    /// Creates an empty variable collection.
    pub const fn new() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    /// Inserts or replaces a named value.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        value: VariableValue,
    ) -> Option<VariableValue> {
        self.values.insert(name.into(), value)
    }

    /// Returns a named value.
    pub fn get(&self, name: &str) -> Option<&VariableValue> {
        self.values.get(name)
    }
}
