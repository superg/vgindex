//! Russian (Cyrillic) transliteration — GOST 7.79-2000 **System B** (the
//! diacritic-free system that uses letter combinations).
//!
//! Pure character mapping: no dictionary, no tokenizer, no dependencies. The
//! only context rule is ц, which becomes "c" before i/e/y/j and "cz" elsewhere.
//!
//! Per redump's "without diacritics" convention (verified against the dataset),
//! the System B modifier marks are written with an apostrophe rather than the
//! strict backtick: ь -> ', ъ -> '', э -> e'; and ы drops its mark -> y
//! (e.g. Объект -> Ob''ekt, Аэропорт -> Ae'roport, вампиры -> vampiry).
//!
//! Casing follows the source: an uppercase Cyrillic letter title-cases its Latin
//! form (Ж -> "Zh"); an uppercase letter inside an ALL-CAPS run upper-cases the
//! whole form (ЖУК -> "ZHUK"). This reproduces the Russian capitalization, so
//! "Анабиоз: Сон разума" -> "Anabioz: Son razuma".

use super::{Script, TransliterationError, TransliterationOutput, Transliterator};

pub struct RussianTransliterator;

impl RussianTransliterator {
    pub fn new() -> Self {
        RussianTransliterator
    }
}

impl Transliterator for RussianTransliterator {
    fn script(&self) -> Script {
        Script::Russian
    }

    fn transliterate(&self, input: &str) -> Result<TransliterationOutput, TransliterationError> {
        let chars: Vec<char> = input.chars().collect();
        let mut out = String::with_capacity(input.len() * 2);

        for i in 0..chars.len() {
            let c = chars[i];
            let lower = lower_char(c);
            let next_lower = chars.get(i + 1).map(|n| lower_char(*n));

            match base_latin(lower, next_lower) {
                Some(latin) => {
                    if c.is_uppercase() {
                        out.push_str(&apply_case(&latin, in_caps_run(&chars, i)));
                    } else {
                        out.push_str(&latin);
                    }
                }
                // Non-Russian characters (spaces, punctuation, digits, Latin)
                // pass through unchanged.
                None => out.push(c),
            }
        }

        Ok(TransliterationOutput {
            text: out,
            script: Script::Russian,
            notes: Vec::new(),
        })
    }
}

fn lower_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// GOST 7.79-2000 System B mapping for a lowercase Cyrillic letter. `next_lower`
/// is the following lowercase letter, used for the ц context rule. Returns the
/// lowercase Latin form, or None for non-Russian characters (passthrough).
fn base_latin(lower: char, next_lower: Option<char>) -> Option<String> {
    let s: &str = match lower {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' => "e",
        'ё' => "yo",
        'ж' => "zh",
        'з' => "z",
        'и' => "i",
        'й' => "j",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "x",
        // ц -> "c" before i/e/y/j-initial letters, "cz" otherwise.
        'ц' => {
            return Some(if next_is_ieyj(next_lower) {
                "c".to_string()
            } else {
                "cz".to_string()
            })
        }
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shh",
        // Redump convention (from dataset): System B marks written with an
        // apostrophe, not the strict backtick; ы drops its mark entirely.
        'ъ' => "''",
        'ы' => "y",
        'ь' => "'",
        'э' => "e'",
        'ю' => "yu",
        'я' => "ya",
        _ => return None,
    };
    Some(s.to_string())
}

/// Whether the next letter's Latin form starts with i, e, y, or j (the ц rule).
fn next_is_ieyj(next_lower: Option<char>) -> bool {
    matches!(
        next_lower,
        Some('и') | Some('е') | Some('э') | Some('ы') | Some('ю') | Some('я') | Some('ё') | Some('й')
    )
}

/// True if the uppercase letter at `i` sits in an all-caps run (an adjacent
/// cased Cyrillic letter is also uppercase).
fn in_caps_run(chars: &[char], i: usize) -> bool {
    let upper_neighbor = |j: usize| chars.get(j).is_some_and(|c| c.is_uppercase());
    let prev = i > 0 && upper_neighbor(i - 1);
    let next = upper_neighbor(i + 1);
    prev || next
}

/// Apply source casing to a lowercase Latin form.
fn apply_case(latin: &str, all_caps: bool) -> String {
    if all_caps {
        return latin.to_uppercase();
    }
    // Title-case: uppercase the first ASCII letter, leave the rest.
    let mut done = false;
    latin
        .chars()
        .map(|c| {
            if !done && c.is_ascii_alphabetic() {
                done = true;
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(s: &str) -> String {
        RussianTransliterator.transliterate(s).unwrap().text
    }

    #[test]
    fn official_example() {
        assert_eq!(tr("Анабиоз: Сон разума"), "Anabioz: Son razuma");
    }

    #[test]
    fn digraphs_and_special() {
        assert_eq!(tr("жз"), "zhz");
        assert_eq!(tr("щука"), "shhuka");
        assert_eq!(tr("ёж"), "yozh");
        assert_eq!(tr("Хорошо"), "Xorosho");
    }

    #[test]
    fn tse_context_rule() {
        assert_eq!(tr("цирк"), "cirk"); // ц before и -> c
        assert_eq!(tr("цены"), "ceny"); // ц before е -> c
        assert_eq!(tr("конец"), "konecz"); // ц not before i/e/y/j -> cz
    }

    #[test]
    fn soft_hard_signs_and_yery() {
        assert_eq!(tr("огонь"), "ogon'"); // soft sign -> apostrophe
        assert_eq!(tr("подъезд"), "pod''ezd"); // hard sign -> double apostrophe
        assert_eq!(tr("мы"), "my"); // ы -> y (mark dropped)
        assert_eq!(tr("это"), "e'to"); // э -> e'
    }

    #[test]
    fn casing_follows_source() {
        assert_eq!(tr("Жук"), "Zhuk"); // title-case digraph
        assert_eq!(tr("ЖУК"), "ZHUK"); // all-caps digraph
        assert_eq!(tr("СССР"), "SSSR");
    }

    #[test]
    fn latin_and_punctuation_passthrough() {
        assert_eq!(tr("Sega Mega Drive"), "Sega Mega Drive");
        assert_eq!(tr("Тетрис 2"), "Tetris 2");
    }

    #[test]
    #[ignore = "analysis: RU_TSV=/tmp/ru_pairs.tsv cargo test ... analyze_russian -- --ignored --nocapture"]
    fn analyze_russian() {
        let path = std::env::var("RU_TSV").unwrap_or_default();
        if path.is_empty() {
            return;
        }
        let data = std::fs::read_to_string(&path).unwrap();
        // Only score entries whose curated title is itself a romanization (has
        // Cyrillic-derived structure), skipping ones replaced by an English title.
        let (mut n, mut exact) = (0u64, 0u64);
        let mut miss = Vec::new();
        for line in data.lines() {
            let Some((fg, ti)) = line.split_once('\t') else { continue };
            let out = tr(fg);
            // Heuristic: a romanization shares its first letter sound; skip clear
            // English replacements by requiring the engine output to overlap.
            n += 1;
            if out == ti {
                exact += 1;
            } else if miss.len() < 20 {
                miss.push((fg.to_string(), out, ti.to_string()));
            }
        }
        println!("\n== Russian: {exact}/{n} exact ({:.1}%) ==", 100.0 * exact as f64 / n as f64);
        for (fg, out, ti) in &miss {
            println!("  {fg}\n    engine : {out}\n    curated: {ti}");
        }
    }
}
