//! Greek transliteration — expands each Greek letter to its **English name**
//! (α -> Alpha, Σ -> Sigma, Φ -> Phi …).
//!
//! In this catalog Greek letters appear almost exclusively as *symbols* in
//! otherwise-Latin/Japanese titles (e.g. "Super Robot Taisen α" -> "…Alpha",
//! "Ninja Gaiden Σ" -> "…Sigma"), so the useful behavior is letter-name
//! expansion rather than phonetic romanization. Names are always capitalized.
//! Non-Greek characters pass through; spacing is inserted so a name never fuses
//! with adjacent alphanumerics ("30Φ" -> "30 Phi").

use super::{Script, TransliterationError, TransliterationOutput, Transliterator};

pub struct GreekTransliterator;

impl GreekTransliterator {
    pub fn new() -> Self {
        GreekTransliterator
    }
}

impl Transliterator for GreekTransliterator {
    fn script(&self) -> Script {
        Script::Greek
    }

    fn transliterate(&self, input: &str) -> Result<TransliterationOutput, TransliterationError> {
        let mut out = String::with_capacity(input.len() * 4);
        let mut prev_was_name = false;

        for c in input.chars() {
            if let Some(name) = letter_name(c) {
                // Space before the name if it would fuse with a letter/digit.
                if out.chars().last().is_some_and(|p| p.is_alphanumeric()) {
                    out.push(' ');
                }
                out.push_str(name);
                prev_was_name = true;
            } else {
                // Space after a name if the next char is a letter/digit.
                if prev_was_name && c.is_alphanumeric() {
                    out.push(' ');
                }
                out.push(c);
                prev_was_name = false;
            }
        }

        Ok(TransliterationOutput {
            text: out,
            script: Script::Greek,
            notes: Vec::new(),
        })
    }
}

/// English name of a Greek letter (case-insensitive, accents folded). None for
/// non-Greek characters.
fn letter_name(c: char) -> Option<&'static str> {
    let lower = c.to_lowercase().next().unwrap_or(c);
    Some(match lower {
        'α' | 'ά' => "Alpha",
        'β' => "Beta",
        'γ' => "Gamma",
        'δ' => "Delta",
        'ε' | 'έ' => "Epsilon",
        'ζ' => "Zeta",
        'η' | 'ή' => "Eta",
        'θ' => "Theta",
        'ι' | 'ί' | 'ϊ' | 'ΐ' => "Iota",
        'κ' => "Kappa",
        'λ' => "Lambda",
        'μ' => "Mu",
        'ν' => "Nu",
        'ξ' => "Xi",
        'ο' | 'ό' => "Omicron",
        'π' => "Pi",
        'ρ' => "Rho",
        'σ' | 'ς' => "Sigma",
        'τ' => "Tau",
        'υ' | 'ύ' | 'ϋ' | 'ΰ' => "Upsilon",
        'φ' => "Phi",
        'χ' => "Chi",
        'ψ' => "Psi",
        'ω' | 'ώ' => "Omega",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(s: &str) -> String {
        GreekTransliterator.transliterate(s).unwrap().text
    }

    #[test]
    fn single_letter_symbols() {
        assert_eq!(tr("α"), "Alpha");
        assert_eq!(tr("Σ"), "Sigma");
        assert_eq!(tr("Super Robot Taisen α"), "Super Robot Taisen Alpha");
        assert_eq!(tr("Ninja Gaiden Σ"), "Ninja Gaiden Sigma");
    }

    #[test]
    fn spacing_inserted_around_names() {
        assert_eq!(tr("30Φ"), "30 Phi");
        assert_eq!(tr("αβ"), "Alpha Beta");
    }

    #[test]
    fn full_greek_word_expands_each_letter() {
        // Accepted tradeoff: real Greek words become letter sequences.
        assert_eq!(tr("ΓΑΛΕΟΖ"), "Gamma Alpha Lambda Epsilon Omicron Zeta");
    }

    #[test]
    fn passthrough() {
        assert_eq!(tr("Final Fantasy"), "Final Fantasy");
    }
}
