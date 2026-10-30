//! Language selection for every user interface.
//!
//! Translations live in `locales/app.yml` and are compiled into the binary by
//! [`rust_i18n`]. English is the default; the active language is process-wide.
//!
//! The language is resolved in this order (see [`resolve`]):
//! 1. an explicit choice, e.g. the `--lang` CLI flag,
//! 2. the `FERRY_LANG` environment variable,
//! 3. the `language` field of [`Config`](crate::config::Config),
//! 4. [`Language::English`].

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A supported user interface language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English (default).
    #[default]
    #[serde(rename = "en")]
    English,
    /// Polish.
    #[serde(rename = "pl")]
    Polish,
}

impl Language {
    /// All supported languages, in display order.
    pub const ALL: [Language; 2] = [Language::English, Language::Polish];

    /// ISO 639-1 code used in configuration files and locale lookups.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Polish => "pl",
        }
    }

    /// Name of the language in that language, for language pickers.
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Polish => "Polski",
        }
    }

    /// The next language in [`Language::ALL`], wrapping around.
    pub fn next(self) -> Language {
        let index = Self::ALL.iter().position(|l| *l == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Error returned when a language code is not supported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownLanguage(pub String);

impl fmt::Display for UnknownLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let supported: Vec<&str> = Language::ALL.iter().map(|l| l.code()).collect();
        write!(
            f,
            "unsupported language '{}' (supported: {})",
            self.0,
            supported.join(", ")
        )
    }
}

impl std::error::Error for UnknownLanguage {}

impl FromStr for Language {
    type Err = UnknownLanguage;

    /// Parses a language code; region suffixes are ignored (`pl_PL.UTF-8` → Polish).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let code = s
            .trim()
            .split(['_', '-', '.'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        Language::ALL
            .into_iter()
            .find(|l| l.code() == code)
            .ok_or_else(|| UnknownLanguage(s.trim().to_string()))
    }
}

/// Makes `language` the active language of the whole process.
pub fn set_language(language: Language) {
    rust_i18n::set_locale(language.code());
}

/// The currently active language.
pub fn current() -> Language {
    rust_i18n::locale().parse().unwrap_or_default()
}

/// Resolves the language from an explicit choice, `FERRY_LANG` and the saved setting.
///
/// Unsupported values are skipped, so a typo never prevents the app from starting.
pub fn resolve(explicit: Option<&str>, configured: Option<&str>) -> Language {
    let from_env = std::env::var("FERRY_LANG").ok();
    [explicit, from_env.as_deref(), configured]
        .into_iter()
        .flatten()
        .find_map(|code| code.parse().ok())
        .unwrap_or_default()
}

/// Resolves the language (see [`resolve`]) and activates it.
pub fn init(explicit: Option<&str>, configured: Option<&str>) -> Language {
    let language = resolve(explicit, configured);
    set_language(language);
    language
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codes_and_locales() {
        assert_eq!("en".parse(), Ok(Language::English));
        assert_eq!("PL".parse(), Ok(Language::Polish));
        assert_eq!("pl_PL.UTF-8".parse(), Ok(Language::Polish));
        assert!("de".parse::<Language>().is_err());
    }

    #[test]
    fn cycles_through_languages() {
        assert_eq!(Language::English.next(), Language::Polish);
        assert_eq!(Language::Polish.next(), Language::English);
    }

    #[test]
    fn every_key_is_translated_in_both_languages() {
        let source = include_str!("../locales/app.yml");
        let mut missing = Vec::new();
        let mut current: Option<(&str, bool, bool)> = None;
        let mut check = |entry: Option<(&str, bool, bool)>| {
            if let Some((key, en, pl)) = entry
                && !(en && pl)
            {
                missing.push(key.to_string());
            }
        };
        for line in source.lines() {
            if line.starts_with(char::is_alphanumeric) && line.trim_end().ends_with(':') {
                check(current.take());
                current = Some((line.trim_end_matches(':'), false, false));
            } else if let Some(entry) = current.as_mut() {
                let trimmed = line.trim_start();
                entry.1 |= trimmed.starts_with("en:");
                entry.2 |= trimmed.starts_with("pl:");
            }
        }
        check(current);
        assert!(missing.is_empty(), "missing translations: {missing:?}");
    }
}
