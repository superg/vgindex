//! Chinese transliteration backend: Hanyu Pinyin with tone marks dropped.
//!
//! This is intentionally selected by release context (China/Taiwan region), not
//! by auto-detecting Han text. Han-only titles can also be Japanese, so automatic
//! detection remains conservative in `detect.rs`.
//!
//! The catalog helper uses a deterministic house style: each Han character is
//! rendered as one title-cased pinyin syllable, separated by spaces. This avoids
//! guessing Chinese word segmentation while matching the broad style already
//! seen in many imported titles.

use pinyin::ToPinyin;

use super::{Script, TransliterationError, TransliterationOutput, Transliterator};

pub struct ChineseTransliterator;

impl ChineseTransliterator {
    pub fn new() -> Self {
        ChineseTransliterator
    }
}

impl Transliterator for ChineseTransliterator {
    fn script(&self) -> Script {
        Script::Chinese
    }

    fn transliterate(&self, input: &str) -> Result<TransliterationOutput, TransliterationError> {
        let mut out = String::with_capacity(input.len() * 2);
        let mut had_unknown_han = false;
        let mut prev = Boundary::Start;

        for raw in input.chars() {
            if raw.is_whitespace() {
                push_space(&mut out);
                prev = Boundary::Space;
                continue;
            }

            if is_han(raw) {
                match raw.to_pinyin() {
                    Some(py) => {
                        push_pinyin_word(&mut out, prev, &capitalize_ascii(py.plain()));
                        prev = Boundary::PinyinWord;
                    }
                    None => {
                        push_pinyin_word(&mut out, prev, &raw.to_string());
                        had_unknown_han = true;
                        prev = Boundary::PinyinWord;
                    }
                }
                continue;
            }

            let ch = normalize_char(raw);
            if ch.is_ascii_alphanumeric() {
                push_ascii_char(&mut out, prev, ch);
                prev = Boundary::AsciiWord;
            } else {
                push_punctuation(&mut out, ch);
                prev = Boundary::Punctuation(ch);
            }
        }

        let text = collapse_spaces(out.trim());
        let notes = if had_unknown_han {
            vec!["Some Han characters were not found in the pinyin dictionary.".to_string()]
        } else {
            Vec::new()
        };

        Ok(TransliterationOutput {
            text,
            script: Script::Chinese,
            notes,
        })
    }
}

#[derive(Clone, Copy)]
enum Boundary {
    Start,
    PinyinWord,
    AsciiWord,
    Space,
    Punctuation(char),
}

fn is_han(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0x2CEB0..=0x2EBEF
            | 0x30000..=0x3134F
    )
}

fn push_pinyin_word(out: &mut String, prev: Boundary, word: &str) {
    if needs_space_before_pinyin(out, prev) {
        out.push(' ');
    }
    out.push_str(word);
}

fn needs_space_before_pinyin(out: &str, prev: Boundary) -> bool {
    if out.is_empty() || matches!(prev, Boundary::Start | Boundary::Space) {
        return false;
    }
    !matches!(prev, Boundary::Punctuation('(' | '[' | '{' | '"' | '\''))
}

fn push_ascii_char(out: &mut String, prev: Boundary, ch: char) {
    if matches!(prev, Boundary::PinyinWord) {
        out.push(' ');
    }
    out.push(ch);
}

fn push_space(out: &mut String) {
    if !out.is_empty() && !out.ends_with(' ') {
        out.push(' ');
    }
}

fn push_punctuation(out: &mut String, ch: char) {
    match ch {
        ')' | ']' | '}' | ':' | ';' | ',' | '.' | '!' | '?' => {
            trim_trailing_space(out);
            out.push(ch);
            if matches!(ch, ':' | ';' | ',' | '.') {
                out.push(' ');
            }
        }
        '-' | '/' | '\'' | '"' => {
            trim_trailing_space(out);
            out.push(ch);
        }
        '(' | '[' | '{' => {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push(ch);
        }
        _ => {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push(ch);
            out.push(' ');
        }
    }
}

fn trim_trailing_space(out: &mut String) {
    while out.ends_with(' ') {
        out.pop();
    }
}

fn normalize_char(c: char) -> char {
    let u = c as u32;
    match c {
        '\u{3000}' => ' ',
        '\u{3001}' => ',',
        '\u{3002}' => '.',
        '\u{3008}' | '\u{300A}' | '\u{300C}' | '\u{300E}' | '\u{3010}' => '[',
        '\u{3009}' | '\u{300B}' | '\u{300D}' | '\u{300F}' | '\u{3011}' => ']',
        '\u{FF08}' => '(',
        '\u{FF09}' => ')',
        '\u{FF0C}' => ',',
        '\u{FF0D}' => '-',
        '\u{FF0E}' => '.',
        '\u{FF0F}' => '/',
        '\u{FF1A}' => ':',
        '\u{FF1B}' => ';',
        '\u{FF01}' => '!',
        '\u{FF1F}' => '?',
        '\u{FF21}'..='\u{FF3A}' => char::from_u32(u - 0xFEE0).unwrap_or(c),
        '\u{FF41}'..='\u{FF5A}' => char::from_u32(u - 0xFEE0).unwrap_or(c),
        '\u{FF10}'..='\u{FF19}' => char::from_u32(u - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

fn capitalize_ascii(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(s: &str) -> String {
        ChineseTransliterator.transliterate(s).unwrap().text
    }

    #[test]
    fn pinyin_without_tone_marks() {
        assert_eq!(tr("中国"), "Zhong Guo");
        assert_eq!(tr("天地之門"), "Tian Di Zhi Men");
        assert_eq!(tr("看我龍顯神威"), "Kan Wo Long Xian Shen Wei");
    }

    #[test]
    fn keeps_existing_latin_and_numbers() {
        assert_eq!(tr("三國志 2"), "San Guo Zhi 2");
        assert_eq!(tr("Game 2000 中文版"), "Game 2000 Zhong Wen Ban");
    }

    #[test]
    fn normalizes_common_fullwidth_punctuation() {
        assert_eq!(tr("古劍奇譚：貳"), "Gu Jian Qi Tan: Er");
        assert_eq!(tr("汪達與巨像（中文版）"), "Wang Da Yu Ju Xiang (Zhong Wen Ban)");
    }
}
