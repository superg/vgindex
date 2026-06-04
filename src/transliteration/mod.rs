//! Transliteration subsystem.
//!
//! Converts text written in a non-Latin script into a Latin-script *draft*
//! suitable for a Main Title. The public surface is script-agnostic:
//!
//! - [`Transliterator`] -- a backend for one writing system.
//! - [`TransliterationRegistry`] -- owns the backends, detects the script (or
//!   takes an explicit one), and dispatches.
//! - [`Script`], [`TransliterationOutput`], [`TransliterationError`] -- shared types.
//!
//! Japanese (modified Hepburn) is the first backend. To add another script
//! (Chinese, Greek, Cyrillic, ...): implement [`Transliterator`], add a variant
//! to [`Script`], register it in [`TransliterationRegistry::new`], and extend
//! `detect::detect_script`.

mod detect;
pub mod japanese;

pub use detect::detect_script;

use std::collections::HashMap;

/// A supported writing system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Script {
    Japanese,
    // future: Chinese, Greek, Cyrillic, ...
}

impl Script {
    pub fn as_str(self) -> &'static str {
        match self {
            Script::Japanese => "japanese",
        }
    }
}

/// Result of a transliteration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TransliterationOutput {
    /// The Latin-script draft.
    pub text: String,
    /// Which script backend produced it.
    pub script: Script,
    /// Non-fatal hints for the user (e.g. "review word spacing").
    pub notes: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TransliterationError {
    #[error("input contained no transliterable text")]
    EmptyInput,
    #[error("no transliterator available for the requested script")]
    UnsupportedScript,
    #[error("transliteration backend error: {0}")]
    Backend(String),
}

/// A backend that transliterates one writing system.
pub trait Transliterator: Send + Sync {
    fn script(&self) -> Script;
    fn transliterate(&self, input: &str) -> Result<TransliterationOutput, TransliterationError>;
}

/// Owns the available backends and dispatches by script. Construct once at
/// startup (backends may load dictionaries) and share via `Arc`.
pub struct TransliterationRegistry {
    backends: HashMap<Script, Box<dyn Transliterator>>,
}

impl TransliterationRegistry {
    /// Build the registry, constructing every backend. Returns an error if a
    /// backend fails to initialize (e.g. dictionary load failure).
    pub fn new() -> Result<Self, TransliterationError> {
        let mut backends: HashMap<Script, Box<dyn Transliterator>> = HashMap::new();

        let japanese = japanese::JapaneseTransliterator::new()?;
        backends.insert(Script::Japanese, Box::new(japanese));

        Ok(Self { backends })
    }

    /// Whether a backend exists for `script`.
    pub fn supports(&self, script: Script) -> bool {
        self.backends.contains_key(&script)
    }

    /// Transliterate `input`. If `script` is `None`, detect it from the text.
    pub fn transliterate(
        &self,
        input: &str,
        script: Option<Script>,
    ) -> Result<TransliterationOutput, TransliterationError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(TransliterationError::EmptyInput);
        }
        let script = match script.or_else(|| detect_script(trimmed)) {
            Some(s) => s,
            None => return Err(TransliterationError::UnsupportedScript),
        };
        let backend = self
            .backends
            .get(&script)
            .ok_or(TransliterationError::UnsupportedScript)?;
        backend.transliterate(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_dispatches_japanese() {
        let reg = TransliterationRegistry::new().expect("registry");
        assert!(reg.supports(Script::Japanese));
        let out = reg.transliterate("恋愛", None).unwrap();
        assert_eq!(out.text, "Ren'ai");
    }

    #[test]
    fn empty_and_latin_are_errors() {
        let reg = TransliterationRegistry::new().expect("registry");
        assert!(matches!(
            reg.transliterate("   ", None),
            Err(TransliterationError::EmptyInput)
        ));
        assert!(matches!(
            reg.transliterate("Dracula", None),
            Err(TransliterationError::UnsupportedScript)
        ));
    }
}
