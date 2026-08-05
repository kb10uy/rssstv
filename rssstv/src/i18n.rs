use std::{borrow::Cow, fmt};

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

const EN: &str = include_str!("../locales/en.ftl");
const JA: &str = include_str!("../locales/ja.ftl");

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Locale {
    #[default]
    En,
    Ja,
}

impl Locale {
    pub const ALL: [Self; 2] = [Self::En, Self::Ja];

    const fn source(self) -> &'static str {
        match self {
            Self::En => EN,
            Self::Ja => JA,
        }
    }

    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ja => "ja",
        }
    }

    /// Resolves a stored language tag, tolerating a hand-edited difference in
    /// case.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|locale| locale.tag().eq_ignore_ascii_case(tag))
    }

    fn identifier(self) -> LanguageIdentifier {
        self.tag().parse().expect("locale tag is well formed")
    }
}

impl fmt::Display for Locale {
    /// Language names are endonyms and are deliberately not translated.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::En => "English",
            Self::Ja => "日本語",
        })
    }
}

pub struct I18n {
    locale: Locale,
    bundle: FluentBundle<FluentResource>,
}

impl fmt::Debug for I18n {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("I18n")
            .field("locale", &self.locale)
            .finish_non_exhaustive()
    }
}

impl I18n {
    pub fn new(locale: Locale) -> Self {
        let resource = FluentResource::try_new(locale.source().to_owned())
            .expect("bundled locale resource parses");
        let mut bundle = FluentBundle::new(vec![locale.identifier()]);
        bundle.set_use_isolating(false);
        bundle
            .add_resource(resource)
            .expect("bundled locale resource has no conflicting messages");
        Self { locale, bundle }
    }

    pub const fn locale(&self) -> Locale {
        self.locale
    }

    pub fn text(&self, key: &str) -> String {
        self.format(key, None)
    }

    pub fn text_with(&self, key: &str, args: &[(&str, Value<'_>)]) -> String {
        let mut arguments = FluentArgs::new();
        for (name, value) in args {
            arguments.set(*name, value.clone());
        }
        self.format(key, Some(&arguments))
    }

    fn format(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        let Some(message) = self.bundle.get_message(key) else {
            return key.to_owned();
        };
        let Some(pattern) = message.value() else {
            return key.to_owned();
        };
        let mut errors = Vec::new();
        self.bundle
            .format_pattern(pattern, args, &mut errors)
            .into_owned()
    }
}

pub type Value<'a> = FluentValue<'a>;

/// Passes borrowed text to a message.
pub fn arg(value: &str) -> Value<'_> {
    Value::String(Cow::Borrowed(value))
}

/// Passes text a message has to own, such as a formatted error.
pub fn owned(value: String) -> Value<'static> {
    Value::String(Cow::Owned(value))
}

/// Passes a number to a message, formatted for the locale.
pub fn number(value: impl Into<f64>) -> Value<'static> {
    Value::from(value.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(Locale::En)]
    #[case(Locale::Ja)]
    fn resources_parse_and_resolve(#[case] locale: Locale) {
        let i18n = I18n::new(locale);
        assert_eq!(i18n.locale(), locale);
        assert_ne!(i18n.text("tab-receive"), "tab-receive");
    }

    fn message_keys(locale: Locale) -> BTreeSet<String> {
        locale
            .source()
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .filter(|(key, _)| !key.is_empty() && !key.starts_with([' ', '#', '.', '*', '[']))
            .map(|(key, _)| key.to_owned())
            .collect()
    }

    #[test]
    fn every_locale_defines_the_same_keys() {
        let reference = message_keys(Locale::default());
        assert!(!reference.is_empty());
        for locale in Locale::ALL {
            assert_eq!(
                message_keys(locale),
                reference,
                "{locale:?} does not match the default locale"
            );
        }
    }

    #[test]
    fn arguments_are_substituted_without_isolation_marks() {
        let formatted =
            I18n::new(Locale::En).text_with("state-receiving", &[("percent", number(94))]);
        assert_eq!(formatted, "RECEIVING \u{b7} 94%");
    }

    #[test]
    fn missing_keys_fall_back_to_the_key() {
        assert_eq!(I18n::new(Locale::En).text("no-such-key"), "no-such-key");
    }

    /// Collects every key the application asks for by name.
    ///
    /// A key that is not defined is answered with the key itself, so a
    /// mistyped one reaches the operator as `station-titel` rather than as a
    /// failure. Nothing else notices, which is why this reads the call sites.
    fn requested_keys() -> BTreeSet<String> {
        fn walk(directory: &std::path::Path, keys: &mut BTreeSet<String>) {
            for entry in std::fs::read_dir(directory).expect("the source tree is readable") {
                let path = entry.expect("the source tree is readable").path();
                if path.is_dir() {
                    walk(&path, keys);
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    // This module defines the lookup rather than reaching it,
                    // and scanning it would find this scanner's own strings.
                    && path.file_name().is_some_and(|name| name != "i18n.rs")
                {
                    collect(
                        &std::fs::read_to_string(&path).expect("a source file"),
                        keys,
                    );
                }
            }
        }

        // `text("key")` and `text_with("key", ...)`, which is every way the
        // bundle is reached, plus the `label_key` arms that answer with one.
        fn collect(source: &str, keys: &mut BTreeSet<String>) {
            for (index, _) in source
                .match_indices("text(")
                .chain(source.match_indices("text_with("))
            {
                let rest = &source[index..];
                let Some(open) = rest.find('(') else { continue };
                let Some(quoted) = rest[open + 1..].strip_prefix('"') else {
                    continue;
                };
                let Some(end) = quoted.find('"') else {
                    continue;
                };
                keys.insert(quoted[..end].to_owned());
            }
        }

        let mut keys = BTreeSet::new();
        walk(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .as_path(),
            &mut keys,
        );
        keys
    }

    #[test]
    fn every_requested_key_is_defined() {
        let defined = message_keys(Locale::default());
        let requested = requested_keys();
        assert!(
            requested.len() > 50,
            "the call sites should have been found: {requested:?}"
        );
        let missing = requested.difference(&defined).collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "keys asked for but not defined: {missing:?}"
        );
    }

    /// Every label a menu or a control is named by has to resolve.
    ///
    /// These reach the bundle through a `label_key` rather than by a literal
    /// at the call site, so the check above does not see them.
    #[test]
    fn every_label_key_is_defined() {
        let defined = message_keys(Locale::default());
        let labels = crate::app::Tab::ALL
            .map(crate::app::Tab::label_key)
            .into_iter()
            .chain(crate::app::Dsp::ALL.map(crate::app::Dsp::label_key))
            .chain(
                crate::storage::history::HistoryFormat::ALL
                    .map(crate::storage::history::HistoryFormat::label_key),
            )
            .chain(crate::storage::paths::Folder::ALL.map(crate::storage::paths::Folder::label_key))
            .chain(
                [
                    crate::worker::rig::RigState::Disconnected,
                    crate::worker::rig::RigState::Connecting,
                    crate::worker::rig::RigState::Receiving,
                    crate::worker::rig::RigState::Transmitting,
                    crate::worker::rig::RigState::Failed,
                ]
                .map(crate::worker::rig::RigState::label_key),
            );
        for label in labels {
            assert!(defined.contains(label), "`{label}` is not defined");
        }
    }
}
