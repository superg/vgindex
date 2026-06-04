//! Lightweight writing-system detection.
//!
//! Used when the caller doesn't specify a script explicitly. Counts characters
//! by Unicode block and returns the dominant *supported* script, or `None` if
//! the text has nothing transliterable (e.g. it's already Latin).
//!
//! As new backends are added (Greek, Cyrillic, Chinese), extend `Counts` and the
//! resolution logic here.

use super::Script;

#[derive(Default)]
struct Counts {
    kana: usize,
    han: usize,
    // future: greek, cyrillic, ...
}

fn tally(input: &str) -> Counts {
    let mut c = Counts::default();
    for ch in input.chars() {
        let u = ch as u32;
        match u {
            // Hiragana + Katakana (+ Katakana phonetic extensions).
            0x3040..=0x30FF | 0x31F0..=0x31FF => c.kana += 1,
            // CJK Unified Ideographs (+ Extension A).
            0x3400..=0x4DBF | 0x4E00..=0x9FFF => c.han += 1,
            _ => {}
        }
    }
    c
}

/// Detect the dominant supported script, or `None` if nothing is transliterable.
pub fn detect_script(input: &str) -> Option<Script> {
    let c = tally(input);
    // Any kana is a strong Japanese signal. Han-only is ambiguous (could be
    // Chinese); until a Chinese backend exists we treat CJK as Japanese, which
    // matches this project's catalog focus.
    if c.kana > 0 || c.han > 0 {
        Some(Script::Japanese)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_japanese() {
        assert_eq!(detect_script("悪魔城ドラキュラ"), Some(Script::Japanese));
        assert_eq!(detect_script("ひらがな"), Some(Script::Japanese));
        assert_eq!(detect_script("漢字"), Some(Script::Japanese));
    }

    #[test]
    fn latin_is_not_transliterable() {
        assert_eq!(detect_script("Dracula"), None);
        assert_eq!(detect_script("Final Fantasy VII"), None);
    }
}
