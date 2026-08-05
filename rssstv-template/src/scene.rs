use std::{collections::BTreeMap, fmt};

use jiff::Zoned;

use crate::renderer::variable::{DEFAULT_TIMESTAMP_FORMAT, references};

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

    /// Whether any text layer reads a timestamp out of `variables`.
    ///
    /// A composition that shows the time stops being true a minute after it
    /// was made, so a caller that keeps a rendered overlay around asks this to
    /// find out whether it has to render again as the clock moves.
    pub fn uses_timestamps(&self, variables: &Variables) -> bool {
        fn any(layers: &[Layer], variables: &Variables) -> bool {
            layers.iter().any(|layer| match layer {
                Layer::Text(text) => references(&text.text)
                    .any(|name| matches!(variables.get(name), Some(VariableValue::Timestamp(_)))),
                Layer::Group(group) => any(&group.layers, variables),
                _ => false,
            })
        }
        any(&self.layers, variables)
    }

    /// Whether any text layer reads what the radio is tuned to.
    ///
    /// Asked for the same reason as [`Template::uses_timestamps`]: a
    /// composition that prints the frequency stops being true the moment the
    /// operator tunes, and only a caller that knows the template reads it has
    /// any reason to compose again when the rig moves.
    ///
    /// Named rather than compared against a value, because the frequency and
    /// the band are ordinary text and a number: there is nothing in what they
    /// hold that says where they came from.
    pub fn uses_radio(&self) -> bool {
        fn any(layers: &[Layer]) -> bool {
            layers.iter().any(|layer| match layer {
                Layer::Text(text) => {
                    references(&text.text).any(|name| name.starts_with(RADIO_PREFIX))
                }
                Layer::Group(group) => any(&group.layers),
                _ => false,
            })
        }
        any(&self.layers)
    }
}

/// What every variable the radio fills in is named under.
const RADIO_PREFIX: &str = "radio.";

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

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    Solid(Color),
    Gradient(Gradient),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Gradient {
    pub kind: GradientKind,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GradientKind {
    Linear { angle: f64 },
    Radial,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GradientStop {
    pub offset: f64,
    pub color: Color,
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
    pub(crate) fill: Paint,
    pub(crate) stroke: Option<Stroke>,
}

/// A rectangle layer.
#[derive(Clone, Debug, PartialEq)]
pub struct RectangleLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) fill: Option<Paint>,
    pub(crate) stroke: Option<Stroke>,
}

/// An ellipse layer.
#[derive(Clone, Debug, PartialEq)]
pub struct EllipseLayer {
    pub(crate) position: Position,
    pub(crate) size: LayerSize,
    pub(crate) fill: Option<Paint>,
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
    /// An instant in a named time zone, rendered through a format expression.
    Timestamp(Zoned),
}

impl fmt::Display for VariableValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => formatter.write_str(value),
            Self::Integer(value) => value.fmt(formatter),
            Self::Decimal(value) => value.fmt(formatter),
            Self::Boolean(value) => value.fmt(formatter),
            Self::Timestamp(value) => value.strftime(DEFAULT_TIMESTAMP_FORMAT).fmt(formatter),
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

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    fn variables() -> Variables {
        let mut variables = Variables::new();
        variables.insert("station.callsign", VariableValue::Text("JA1ABC".into()));
        variables.insert(
            "tx.timestamp.utc",
            VariableValue::Timestamp(
                date(2026, 8, 4)
                    .at(9, 5, 0, 0)
                    .in_tz("UTC")
                    .expect("UTC is a known time zone"),
            ),
        );
        variables
    }

    fn text_template(text: &str) -> Template {
        Template::parse(&format!(
            "group {{ text {text:?} {{ position x=(fw)0 y=(fh)0; font family=\"Noto Sans\" size=(fh)9 weight=400; fill color=\"#ffffff\"; }} }}"
        ))
        .expect("the template is well formed")
    }

    /// The clock only has to be watched for a template that shows it, and a
    /// nested layer counts the same as a top-level one.
    #[test]
    fn a_timestamp_in_a_group_is_still_found() {
        let variables = variables();
        assert!(text_template("${tx.timestamp.utc:%H:%M}").uses_timestamps(&variables));
        assert!(!text_template("${station.callsign}").uses_timestamps(&variables));
        assert!(!text_template("plain").uses_timestamps(&variables));
    }

    /// The rig only has to be watched for a template that prints what it is
    /// tuned to, and a nested layer counts the same as a top-level one.
    #[test]
    fn a_radio_variable_in_a_group_is_still_found() {
        assert!(text_template("${radio.frequency:.3}").uses_radio());
        assert!(text_template("${radio.band}").uses_radio());
        assert!(!text_template("${station.callsign}").uses_radio());
        assert!(!text_template("plain").uses_radio());
    }
}
