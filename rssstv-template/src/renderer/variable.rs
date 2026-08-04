use std::fmt::Write;

use crate::{TemplateError, scene::Variables};

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
        let name = &expression[..end];
        if !valid_variable_name(name) {
            return Err(TemplateError::Schema(format!(
                "invalid variable name `{name}`"
            )));
        }
        let value = variables
            .get(name)
            .ok_or_else(|| TemplateError::MissingVariable(name.to_owned()))?;
        write!(output, "{value}").unwrap();
        rest = &expression[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
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
    use super::*;
    use crate::VariableValue;

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
}
