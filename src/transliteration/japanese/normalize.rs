//! Input normalization applied to a Foreign Title before tokenization.
//!
//! These rules clean up characters that commonly appear in Japanese titles so
//! the result is a sensible Latin-script Main Title:
//!
//! - Fullwidth ASCII (letters, digits, punctuation) and the ideographic space
//!   are folded to their normal ASCII forms ("２Ｄ" -> "2D", "　" -> " ").
//! - ’ (U+2019) -> ' (ASCII apostrophe).
//! - Japanese quotation marks 『 』 -> western double quote ".
//! - 【 】 and 「 」 are stripped (ignored).
//! - Unicode Roman numerals Ⅰ..Ⅻ (and L/C/D/M) -> ASCII letters (Ⅶ -> "VII").
//! - ・ (katakana middle dot) -> a space.
//! - Separator dashes/tildes （－ ～ 〜） are resolved against the *monospace*
//!   space 　 (U+3000) around them:
//!     - `　-` / `　～` (monospace space before)  -> ": " (colon + space);
//!     - a separator with no monospace space before -> " " (a plain space);
//!     - `-　` / `～　` (monospace space after, none before) -> removed.
//!   For the wrapped form `　-　`, the "before" rule wins and the trailing
//!   monospace space is absorbed, giving ": ". Only the fullwidth/wave separator
//!   forms are treated this way -- an ASCII '-' (e.g. "Spider-Man") is preserved.
//!
//! The katakana chouonpu ー (U+30FC) is intentionally left untouched -- it is a
//! long-vowel marker handled later by the romaji rules, not a separator.

/// Monospace (ideographic / fullwidth) space.
const MONOSPACE_SPACE: char = '\u{3000}';

/// Title separator dashes/tildes: fullwidth hyphen-minus －, wave dash 〜, and
/// fullwidth tilde ～. ASCII '-'/'~' are deliberately excluded.
fn is_separator(c: char) -> bool {
    matches!(c, '\u{FF0D}' | '\u{301C}' | '\u{FF5E}')
}

/// Unicode Roman numeral character -> ASCII letters. Covers the common uppercase
/// forms Ⅰ..Ⅻ plus L/C/D/M. Lowercase forms (often used as "x"/multiplication)
/// are intentionally left alone.
fn roman_numeral(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{2160}' => "I",
        '\u{2161}' => "II",
        '\u{2162}' => "III",
        '\u{2163}' => "IV",
        '\u{2164}' => "V",
        '\u{2165}' => "VI",
        '\u{2166}' => "VII",
        '\u{2167}' => "VIII",
        '\u{2168}' => "IX",
        '\u{2169}' => "X",
        '\u{216A}' => "XI",
        '\u{216B}' => "XII",
        '\u{216C}' => "L",
        '\u{216D}' => "C",
        '\u{216E}' => "D",
        '\u{216F}' => "M",
        _ => return None,
    })
}

/// Katakana spelling of a single Latin letter (エー -> "A", エックス -> "X").
fn letter_name(s: &str) -> Option<&'static str> {
    Some(match s {
        "エー" => "A",
        "ビー" => "B",
        "シー" => "C",
        "ディー" => "D",
        "イー" => "E",
        "エフ" => "F",
        "ジー" => "G",
        "エイチ" => "H",
        "アイ" => "I",
        "ジェー" | "ジェイ" => "J",
        "ケー" => "K",
        "エル" => "L",
        "エム" => "M",
        "エヌ" => "N",
        "オー" => "O",
        "ピー" => "P",
        "キュー" => "Q",
        "アール" => "R",
        "エス" => "S",
        "ティー" => "T",
        "ユー" => "U",
        "ブイ" => "V",
        "ダブリュー" => "W",
        "エックス" => "X",
        "ワイ" => "Y",
        "ゼット" | "ズィー" => "Z",
        _ => return None,
    })
}

/// Collapse `・`-delimited sequences of katakana letter-names (length >= 2) into
/// the concatenated Latin acronym (ディー・エックス -> "DX"). Sequences that are not
/// entirely letter-names are left untouched for normal handling. Rare in the
/// dataset but unambiguous when the `・` delimiter is present.
fn despell_letters(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    // Split into `・`-delimited groups but keep other text intact: scan runs.
    for segment in split_keep_non_dot_groups(input) {
        let parts: Vec<&str> = segment.split('・').collect();
        if parts.len() >= 2 && parts.iter().all(|p| letter_name(p).is_some()) {
            for p in parts {
                out.push_str(letter_name(p).unwrap());
            }
        } else {
            out.push_str(segment);
        }
    }
    out
}

/// Split `input` into maximal segments that are either a single `・`-joined run of
/// katakana, or any other text, preserving order and content.
fn split_keep_non_dot_groups(input: &str) -> Vec<&str> {
    let bytes: Vec<(usize, char)> = input.char_indices().collect();
    let is_kata = |c: char| {
        matches!(c as u32, 0x30A1..=0x30FA | 0x30FC..=0x30FF) // katakana letters, excludes ・(30FB)
    };
    let mut segs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Try to grow a katakana(・katakana)+ group from i.
        let start = bytes[i].0;
        let mut j = i;
        let mut saw_group = false;
        loop {
            let run_start = j;
            while j < bytes.len() && is_kata(bytes[j].1) {
                j += 1;
            }
            if j == run_start {
                break;
            }
            // optional ・ followed by more katakana
            if j < bytes.len()
                && bytes[j].1 == '・'
                && j + 1 < bytes.len()
                && is_kata(bytes[j + 1].1)
            {
                saw_group = true;
                j += 1;
                continue;
            }
            break;
        }
        if saw_group {
            let end = if j < bytes.len() {
                bytes[j].0
            } else {
                input.len()
            };
            segs.push(&input[start..end]);
            i = j;
        } else {
            // Emit a single char as its own segment.
            let end = if i + 1 < bytes.len() {
                bytes[i + 1].0
            } else {
                input.len()
            };
            segs.push(&input[start..end]);
            i += 1;
        }
    }
    segs
}

/// Normalize a raw Foreign Title string per the rules above.
pub fn normalize(input: &str) -> String {
    let despelled = despell_letters(input);
    let chars: Vec<char> = despelled.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if is_separator(c) {
            let before_mono = i > 0 && chars[i - 1] == MONOSPACE_SPACE;
            let after_mono = i + 1 < chars.len() && chars[i + 1] == MONOSPACE_SPACE;

            if before_mono {
                // "　-" / "　-　" -> ": " (drop the space(s) we already emitted
                // for the leading monospace space, absorb a trailing one).
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push_str(": ");
                if after_mono {
                    i += 1; // absorb trailing monospace space
                }
            } else if after_mono {
                // "-　" -> removed (drop the separator and the trailing space).
                i += 1; // absorb trailing monospace space
            } else {
                // bare separator -> a plain space
                out.push(' ');
            }
            i += 1;
            continue;
        }

        match c {
            '\u{3000}' => out.push(' '),  // ideographic (fullwidth) space
            '\u{2019}' => out.push('\''), // ’ -> '
            '\u{300E}' | '\u{300F}' => out.push('"'), // 『 』 -> "
            '\u{3010}' | '\u{3011}' => {} // 【 】 -> ignore (strip)
            '\u{300C}' | '\u{300D}' => {} // 「 」 -> ignore (strip)
            '\u{30FB}' => out.push(' '),  // ・ -> space
            '\u{3001}' => out.push(','),  // 、 -> ,
            '\u{3002}' => out.push('.'),  // 。 -> .
            // Stray (semi)voiced sound marks used decoratively -> strip.
            '\u{309B}' | '\u{309C}' | '\u{3099}' | '\u{309A}' => {}
            // Unicode Roman numerals -> ASCII letters (Ⅶ -> "VII").
            c if roman_numeral(c).is_some() => out.push_str(roman_numeral(c).unwrap()),
            // Fullwidth ASCII block U+FF01..=U+FF5E -> ASCII (subtract 0xFEE0).
            c if ('\u{FF01}'..='\u{FF5E}').contains(&c) => {
                out.push(char::from_u32(c as u32 - 0xFEE0).unwrap_or(c));
            }
            _ => out.push(c),
        }
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_fullwidth_alnum_and_space() {
        assert_eq!(normalize("２Ｄ"), "2D");
        assert_eq!(normalize("ＲＰＧ"), "RPG");
        assert_eq!(normalize("Ａ　Ｂ"), "A B");
        assert_eq!(normalize("２０２４"), "2024");
    }

    #[test]
    fn apostrophe_and_quotes() {
        assert_eq!(normalize("d’s"), "d's");
        assert_eq!(normalize("『夢』"), "\"夢\"");
    }

    #[test]
    fn brackets_stripped_and_middot_spaced() {
        assert_eq!(normalize("【限定】夢"), "限定夢");
        assert_eq!(normalize("「夢」島"), "夢島");
        assert_eq!(normalize("夢・島"), "夢 島");
    }

    #[test]
    fn separator_monospace_before_becomes_colon() {
        assert_eq!(normalize("夢　－島"), "夢: 島");
        assert_eq!(normalize("夢　～島"), "夢: 島");
        assert_eq!(normalize("夢　〜島"), "夢: 島");
        // Wrapped form 　-　 : "before" wins, trailing monospace space absorbed.
        assert_eq!(normalize("夢　－　島"), "夢: 島");
    }

    #[test]
    fn separator_without_monospace_before_becomes_space() {
        assert_eq!(normalize("夢－島"), "夢 島");
        assert_eq!(normalize("夢～島"), "夢 島");
    }

    #[test]
    fn separator_with_trailing_monospace_is_removed() {
        assert_eq!(normalize("夢－　島"), "夢島");
        assert_eq!(normalize("夢～　島"), "夢島");
    }

    #[test]
    fn ascii_hyphen_is_preserved() {
        assert_eq!(normalize("Spider-Man"), "Spider-Man");
    }

    #[test]
    fn chouonpu_preserved() {
        assert_eq!(normalize("ラーメン"), "ラーメン");
    }

    #[test]
    fn ideographic_comma_and_period() {
        assert_eq!(normalize("武将、城"), "武将,城");
        assert_eq!(normalize("終わり。"), "終わり.");
    }

    #[test]
    fn roman_numerals_to_ascii() {
        assert_eq!(normalize("幻想水滸伝Ⅱ"), "幻想水滸伝II");
        assert_eq!(
            normalize("ファイナルファンタジーⅦ"),
            "ファイナルファンタジーVII"
        );
        assert_eq!(normalize("三國志Ⅲ"), "三國志III");
    }

    #[test]
    fn letter_spelling_via_dot() {
        assert_eq!(normalize("雷電ディー・エックス"), "雷電DX");
        assert_eq!(normalize("エル・オー・エル"), "LOL");
        // A non-letter-name group keeps the middot -> space behavior.
        assert_eq!(normalize("ラウンド・ザ・ワールド"), "ラウンド ザ ワールド");
    }
}
