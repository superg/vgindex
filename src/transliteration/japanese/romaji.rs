//! Pure kana -> romaji conversion (modified Hepburn, title style).
//!
//! This module has **no external dependencies** and is fully unit-tested. It
//! operates on a kana string (hiragana or katakana) -- typically the katakana
//! *reading* of a token produced by the tokenizer, or a kana surface form.
//!
//! It implements the mechanical rules from the Redump title framework:
//! - base syllable table + palatalized (きゃ -> kya) + common foreign (ファ -> fa) combos
//! - sokuon っ/ッ gemination, including the っち -> "tch" exception
//! - chouonpu ー long-vowel repetition (ガーン -> Gaan)
//! - ん/ン -> "n", written "n'" before a vowel or "y" (れんあい -> ren'ai)
//!
//! Word spacing, capitalization, particle handling and overrides live one level
//! up in `japanese::mod`, because they need token/POS context this layer lacks.
//! Non-kana characters (latin, digits, punctuation) pass through unchanged.

/// One logical kana element, produced in pass 1 and assembled in pass 2.
enum Unit {
    /// A romanized syllable (already lowercase romaji).
    Syl(String),
    /// Sokuon っ/ッ -- geminate the following consonant.
    Sokuon,
    /// Chouonpu ー -- repeat the preceding vowel.
    Choon,
    /// Moraic nasal ん/ン.
    N,
    /// Passthrough text that is not kana (latin, digits, symbols).
    Raw(String),
}

/// Convert a kana string to romaji, applying the mechanical Hepburn rules.
pub fn romanize_kana(input: &str) -> String {
    let kata: Vec<char> = to_katakana(input).chars().collect();
    let units = lex(&kata);
    assemble(&units)
}

/// Expand Latin vowels carrying macrons into the title-style double-vowel form
/// (rule #4): ā->aa, ē->ee, ī->ii, ō->ou, ū->uu (case preserving). Useful when an
/// override or surface form already contains macrons.
pub fn expand_macrons(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            'ā' => out.push_str("aa"),
            'ē' => out.push_str("ee"),
            'ī' => out.push_str("ii"),
            'ō' => out.push_str("ou"),
            'ū' => out.push_str("uu"),
            'Ā' => out.push_str("Aa"),
            'Ē' => out.push_str("Ee"),
            'Ī' => out.push_str("Ii"),
            'Ō' => out.push_str("Ou"),
            'Ū' => out.push_str("Uu"),
            _ => out.push(ch),
        }
    }
    out
}

/// Normalize hiragana to katakana so we only maintain one table. Small kana,
/// chouonpu and non-kana are left as-is.
fn to_katakana(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            let u = c as u32;
            // Hiragana block U+3041..=U+3096 -> Katakana by +0x60.
            if (0x3041..=0x3096).contains(&u) {
                char::from_u32(u + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')
}

/// Small vowel kana -> its plain vowel romaji.
fn small_vowel(c: char) -> Option<char> {
    match c {
        'ァ' => Some('a'),
        'ィ' => Some('i'),
        'ゥ' => Some('u'),
        'ェ' => Some('e'),
        'ォ' => Some('o'),
        _ => None,
    }
}

fn small_ya(c: char) -> Option<char> {
    match c {
        'ャ' => Some('a'),
        'ュ' => Some('u'),
        'ョ' => Some('o'),
        _ => None,
    }
}

/// Base katakana syllable -> romaji.
fn kata_base(c: char) -> Option<&'static str> {
    let s = match c {
        'ア' => "a",
        'イ' => "i",
        'ウ' => "u",
        'エ' => "e",
        'オ' => "o",
        'カ' => "ka",
        'キ' => "ki",
        'ク' => "ku",
        'ケ' => "ke",
        'コ' => "ko",
        'ガ' => "ga",
        'ギ' => "gi",
        'グ' => "gu",
        'ゲ' => "ge",
        'ゴ' => "go",
        'サ' => "sa",
        'シ' => "shi",
        'ス' => "su",
        'セ' => "se",
        'ソ' => "so",
        'ザ' => "za",
        'ジ' => "ji",
        'ズ' => "zu",
        'ゼ' => "ze",
        'ゾ' => "zo",
        'タ' => "ta",
        'チ' => "chi",
        'ツ' => "tsu",
        'テ' => "te",
        'ト' => "to",
        'ダ' => "da",
        'ヂ' => "ji",
        'ヅ' => "zu",
        'デ' => "de",
        'ド' => "do",
        'ナ' => "na",
        'ニ' => "ni",
        'ヌ' => "nu",
        'ネ' => "ne",
        'ノ' => "no",
        'ハ' => "ha",
        'ヒ' => "hi",
        'フ' => "fu",
        'ヘ' => "he",
        'ホ' => "ho",
        'バ' => "ba",
        'ビ' => "bi",
        'ブ' => "bu",
        'ベ' => "be",
        'ボ' => "bo",
        'パ' => "pa",
        'ピ' => "pi",
        'プ' => "pu",
        'ペ' => "pe",
        'ポ' => "po",
        'マ' => "ma",
        'ミ' => "mi",
        'ム' => "mu",
        'メ' => "me",
        'モ' => "mo",
        'ヤ' => "ya",
        'ユ' => "yu",
        'ヨ' => "yo",
        'ラ' => "ra",
        'リ' => "ri",
        'ル' => "ru",
        'レ' => "re",
        'ロ' => "ro",
        'ワ' => "wa",
        'ヰ' => "wi",
        'ヱ' => "we",
        'ヲ' => "o",
        'ヴ' => "vu",
        // Small ke/ka used in place names and counters (希望ヶ峰 -> Kibougamine).
        'ヶ' => "ga",
        'ヵ' => "ka",
        _ => return None,
    };
    Some(s)
}

/// Combine a base with a small ya/yu/yo glide. Covers i-row palatalization
/// (きゃ -> kya, しゃ -> sha, ちゃ -> cha, じゃ -> ja) and foreign yōon on other
/// rows (デュ -> dyu, テュ -> tyu, フュ -> fyu). Returns None for a pure-vowel base
/// (no consonant to glide).
fn palatalize(base: &str, glide_vowel: char) -> Option<String> {
    match base {
        "shi" => return Some(format!("sh{glide_vowel}")),
        "chi" => return Some(format!("ch{glide_vowel}")),
        "ji" => return Some(format!("j{glide_vowel}")),
        _ => {}
    }
    // Keep the leading consonant cluster, append "y" + the glide vowel.
    let consonant: String = base.chars().take_while(|c| !is_vowel(*c)).collect();
    if consonant.is_empty() {
        return None;
    }
    Some(format!("{consonant}y{glide_vowel}"))
}

/// Common foreign-sound combos: base + small vowel (ファ -> fa, ティ -> ti, ウィ -> wi).
fn foreign_combo(base_char: char, small: char) -> Option<String> {
    let v = small_vowel(small)?;
    let r = match base_char {
        'フ' => format!("f{v}"),  // ファ fa, フィ fi, フェ fe, フォ fo
        'ヴ' => format!("v{v}"),  // ヴァ va ...
        'ウ' => format!("w{v}"),  // ウィ wi, ウェ we, ウォ wo, ウァ wa
        'ツ' => format!("ts{v}"), // ツァ tsa ...
        'テ' if v == 'i' => "ti".to_string(),
        'デ' if v == 'i' => "di".to_string(),
        'ト' if v == 'u' => "tu".to_string(),
        'ド' if v == 'u' => "du".to_string(),
        'チ' if v == 'e' => "che".to_string(),
        'シ' if v == 'e' => "she".to_string(),
        'ジ' if v == 'e' => "je".to_string(),
        _ => return None,
    };
    Some(r)
}

/// Pass 1: turn the katakana char stream into logical units, resolving 2-char
/// combos (palatal / foreign) with one char of lookahead.
fn lex(chars: &[char]) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            'ッ' => {
                units.push(Unit::Sokuon);
                i += 1;
            }
            'ー' => {
                units.push(Unit::Choon);
                i += 1;
            }
            'ン' => {
                units.push(Unit::N);
                i += 1;
            }
            _ => {
                if let Some(base) = kata_base(c) {
                    // Try a 2-char combo with the next kana.
                    if i + 1 < chars.len() {
                        let next = chars[i + 1];
                        if let Some(gv) = small_ya(next) {
                            if let Some(p) = palatalize(base, gv) {
                                units.push(Unit::Syl(p));
                                i += 2;
                                continue;
                            }
                        }
                        if let Some(fc) = foreign_combo(c, next) {
                            units.push(Unit::Syl(fc));
                            i += 2;
                            continue;
                        }
                    }
                    units.push(Unit::Syl(base.to_string()));
                    i += 1;
                } else if let Some(v) = small_vowel(c) {
                    // Stray small vowel: emit as plain vowel.
                    units.push(Unit::Syl(v.to_string()));
                    i += 1;
                } else if let Some(v) = small_ya(c) {
                    // Stray small ya/yu/yo (no combinable base before it): emit the
                    // glide on its own so the raw kana never leaks (ャ -> ya).
                    units.push(Unit::Syl(format!("y{v}")));
                    i += 1;
                } else {
                    // Non-kana: passthrough (coalesce runs into one Raw).
                    let mut s = String::new();
                    s.push(c);
                    i += 1;
                    units.push(Unit::Raw(s));
                }
            }
        }
    }
    units
}

/// Pass 2: assemble units into a romaji string, applying gemination, long-vowel
/// repetition and the n / n' apostrophe rule.
fn assemble(units: &[Unit]) -> String {
    let mut out = String::new();
    let mut geminate = false;

    for idx in 0..units.len() {
        match &units[idx] {
            Unit::Sokuon => {
                geminate = true;
            }
            Unit::Choon => {
                if let Some(v) = last_vowel(&out) {
                    out.push(v);
                }
            }
            Unit::N => {
                out.push('n');
                // Apostrophe if the next emitted sound starts with a vowel or y.
                if let Some(next) = next_initial(units, idx + 1) {
                    if is_vowel(next) || next == 'y' {
                        out.push('\'');
                    }
                }
            }
            Unit::Syl(s) => {
                if geminate {
                    out.push_str(&geminate_prefix(s));
                    geminate = false;
                }
                out.push_str(s);
            }
            Unit::Raw(s) => {
                geminate = false;
                out.push_str(s);
            }
        }
    }
    out
}

/// The consonant(s) a sokuon prepends before `s`. っち/ッチ -> "tch" (so ち's
/// "chi" becomes "tchi"); otherwise the first consonant letter is doubled.
fn geminate_prefix(s: &str) -> String {
    if s.starts_with("ch") {
        "t".to_string()
    } else {
        match s.chars().next() {
            Some(c0) if !is_vowel(c0) && c0 != 'n' => c0.to_string(),
            _ => String::new(),
        }
    }
}

/// First romaji letter that the unit at `idx` will emit (for the n' rule).
fn next_initial(units: &[Unit], idx: usize) -> Option<char> {
    match units.get(idx)? {
        Unit::Syl(s) => s.chars().next(),
        Unit::Raw(s) => s.chars().next(),
        // A following sokuon/choon/n doesn't trigger the apostrophe.
        _ => None,
    }
}

fn last_vowel(s: &str) -> Option<char> {
    s.chars().rev().find(|c| is_vowel(*c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        romanize_kana(s)
    }

    #[test]
    fn basic_syllables() {
        assert_eq!(r("アクマジョウ"), "akumajou");
        assert_eq!(r("ドコデモイッショ"), "dokodemoissho");
        assert_eq!(r("チバ"), "chiba");
        assert_eq!(r("ケン"), "ken");
    }

    #[test]
    fn palatalized_and_foreign() {
        assert_eq!(r("ドラキュラ"), "dorakyura");
        assert_eq!(r("ファミコン"), "famikon");
        assert_eq!(r("ジャンプ"), "janpu");
        assert_eq!(r("シャイン"), "shain");
    }

    #[test]
    fn foreign_yoon_does_not_leak_raw_kana() {
        // Small ya/yu/yo after a non-i-row base: デュ -> dyu, テュ -> tyu, フュ -> fyu.
        assert_eq!(r("デューティー"), "dyuutii");
        assert_eq!(r("テューバ"), "tyuuba");
        assert_eq!(r("フュージョン"), "fyuujon");
        // No leftover katakana in any output.
        assert!(!r("デューティー").chars().any(|c| (c as u32) >= 0x3040));
    }

    #[test]
    fn chouonpu_repeats_vowel() {
        assert_eq!(r("ガーン"), "gaan");
        assert_eq!(r("ドドーン"), "dodoon");
        assert_eq!(r("ラーメン"), "raamen");
    }

    #[test]
    fn sokuon_gemination() {
        assert_eq!(r("モット"), "motto");
        assert_eq!(r("レッシャ"), "ressha");
        assert_eq!(r("ヨッツ"), "yottsu");
    }

    #[test]
    fn sokuon_tch_exception() {
        assert_eq!(r("マッチャ"), "matcha");
        assert_eq!(r("ボッチ"), "botchi");
    }

    #[test]
    fn moraic_n_apostrophe() {
        assert_eq!(r("レンアイ"), "ren'ai");
        assert_eq!(r("センヨウ"), "sen'you");
        // No apostrophe before a consonant.
        assert_eq!(r("カンジ"), "kanji");
        // Trailing n.
        assert_eq!(r("ニホン"), "nihon");
    }

    #[test]
    fn hiragana_normalized() {
        assert_eq!(r("れっしゃ"), "ressha");
        assert_eq!(r("どこでも"), "dokodemo");
    }

    #[test]
    fn passthrough_non_kana() {
        assert_eq!(r("２Ｄ"), "２Ｄ"); // full-width passthrough (not kana)
        assert_eq!(r("ABC"), "ABC");
    }

    #[test]
    fn macron_expansion() {
        assert_eq!(expand_macrons("Tōkyō"), "Toukyou");
        assert_eq!(expand_macrons("Yūsha"), "Yuusha");
    }
}
