use std::fmt::Write;

use jiff::fmt::strtime;

use crate::{
    TemplateError,
    scene::{VariableValue, Variables},
};

/// How a timestamp is written when a text expression names no format.
pub(crate) const DEFAULT_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M";

pub(super) fn interpolate(source: &str, variables: &Variables) -> Result<String, TemplateError> {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(dollar) = rest.find('$') {
        output.push_str(&rest[..dollar]);
        rest = &rest[dollar + 1..];
        if let Some(after_escape) = rest.strip_prefix('$') {
            output.push('$');
            rest = after_escape;
            continue;
        }
        let Some(expression) = rest.strip_prefix('{') else {
            output.push('$');
            continue;
        };
        let Some(end) = expression.find('}') else {
            return Err(TemplateError::Schema(
                "unterminated variable interpolation".into(),
            ));
        };
        let reference = Reference::parse(&expression[..end])?;
        let value = variables
            .get(reference.name)
            .ok_or_else(|| TemplateError::MissingVariable(reference.name.to_owned()))?;
        reference.write(&mut output, value)?;
        rest = &expression[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// The variable names one text expression reads, in the order they appear.
///
/// Malformed expressions are passed over rather than reported: this walk
/// answers what a template would read, and [`interpolate`] is what refuses to
/// render text it cannot resolve.
pub(crate) fn references(source: &str) -> References<'_> {
    References { rest: source }
}

pub(crate) struct References<'a> {
    rest: &'a str,
}

impl<'a> Iterator for References<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let dollar = self.rest.find('$')?;
            self.rest = &self.rest[dollar + 1..];
            if let Some(after_escape) = self.rest.strip_prefix('$') {
                self.rest = after_escape;
                continue;
            }
            let Some(expression) = self.rest.strip_prefix('{') else {
                continue;
            };
            let Some(end) = expression.find('}') else {
                self.rest = "";
                return None;
            };
            let name = Reference::split(&expression[..end]).0;
            self.rest = &expression[end + 1..];
            return Some(name);
        }
    }
}

/// One `${name}` or `${name:format}` expression.
struct Reference<'a> {
    name: &'a str,
    format: Option<&'a str>,
}

impl<'a> Reference<'a> {
    fn parse(expression: &'a str) -> Result<Self, TemplateError> {
        let (name, format) = Self::split(expression);
        if !valid_variable_name(name) {
            return Err(TemplateError::Schema(format!(
                "invalid variable name `{name}`"
            )));
        }
        if format.is_some_and(str::is_empty) {
            return Err(TemplateError::VariableFormat {
                name: name.to_owned(),
                message: "the format is empty".into(),
            });
        }
        Ok(Self { name, format })
    }

    /// Separates the name from the format without judging either.
    ///
    /// The first colon ends the name, and everything after it is the format,
    /// so a format containing colons such as `%H:%M` needs no escaping.
    fn split(expression: &'a str) -> (&'a str, Option<&'a str>) {
        match expression.split_once(':') {
            Some((name, format)) => (name, Some(format)),
            None => (expression, None),
        }
    }

    fn write(&self, output: &mut String, value: &VariableValue) -> Result<(), TemplateError> {
        match (value, self.format) {
            (VariableValue::Timestamp(zoned), format) => {
                let format = format.unwrap_or(DEFAULT_TIMESTAMP_FORMAT);
                let formatted = strtime::format(format, zoned).map_err(|error| {
                    TemplateError::VariableFormat {
                        name: self.name.to_owned(),
                        message: error.to_string(),
                    }
                })?;
                output.push_str(&formatted);
            }
            (_, Some(_)) => {
                return Err(TemplateError::VariableFormat {
                    name: self.name.to_owned(),
                    message: "only a timestamp takes a format".into(),
                });
            }
            (value, None) => write!(output, "{value}").unwrap(),
        }
        Ok(())
    }
}

fn valid_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
                && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    fn timestamp() -> VariableValue {
        VariableValue::Timestamp(
            date(2026, 8, 4)
                .at(9, 5, 0, 0)
                .in_tz("UTC")
                .expect("UTC is a known time zone"),
        )
    }

    #[test]
    fn interpolates_values_and_escaped_dollars() {
        let mut variables = Variables::new();
        variables.insert("contact.callsign", VariableValue::Text("JA1ABC".into()));
        assert_eq!(
            interpolate("To ${contact.callsign}; $${literal}", &variables).unwrap(),
            "To JA1ABC; ${literal}"
        );
        assert!(matches!(
            interpolate("${station.callsign}", &variables),
            Err(TemplateError::MissingVariable(_))
        ));
    }

    #[test]
    fn formats_a_timestamp_through_the_expression() {
        let mut variables = Variables::new();
        variables.insert("tx.timestamp.utc", timestamp());
        assert_eq!(
            interpolate("${tx.timestamp.utc:%d %b %Y %H:%MZ}", &variables).unwrap(),
            "04 Aug 2026 09:05Z"
        );
    }

    /// A timestamp is still worth writing without a format, so a template that
    /// only wants the date and time does not have to spell one out.
    #[test]
    fn writes_a_timestamp_without_a_format() {
        let mut variables = Variables::new();
        variables.insert("tx.timestamp.utc", timestamp());
        assert_eq!(
            interpolate("${tx.timestamp.utc}", &variables).unwrap(),
            "2026-08-04 09:05"
        );
    }

    #[test]
    fn rejects_an_unusable_format() {
        let mut variables = Variables::new();
        variables.insert("tx.timestamp.utc", timestamp());
        variables.insert("station.callsign", VariableValue::Text("JA1ABC".into()));
        assert!(matches!(
            interpolate("${tx.timestamp.utc:%J}", &variables),
            Err(TemplateError::VariableFormat { .. })
        ));
        assert!(matches!(
            interpolate("${station.callsign:%Y}", &variables),
            Err(TemplateError::VariableFormat { .. })
        ));
        assert!(matches!(
            interpolate("${tx.timestamp.utc:}", &variables),
            Err(TemplateError::VariableFormat { .. })
        ));
    }

    #[test]
    fn lists_the_names_a_text_reads() {
        let names: Vec<_> = references("${a} $${b} plain ${c.d:%H:%M} $x").collect();
        assert_eq!(names, ["a", "c.d"]);
    }
}
