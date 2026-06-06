//! Japanese transliteration backend (modified Hepburn, title style).
//!
//! Pipeline for a Foreign Title:
//! 1. Normalize the input (`normalize`): fullwidth->ASCII, 『』->", 【】 stripped,
//!    ・->space, first －/～ -> ":", ’ -> '.
//! 2. Whole-title override? -> return it verbatim.
//! 3. Segment the normalized text into a sequence of:
//!    - **Override** phrase (matched greedily, may span morphemes) -> verbatim;
//!    - **Japanese** run (kana/kanji) -> tokenize + romanize;
//!    - **Passthrough** run (already-Latin letters/digits/punct/space) -> verbatim.
//! 4. Per Japanese token: word override is handled at the phrase level; particles
//!    (助詞, listed set) are lowercased with は/へ/を fixed; every other token is the
//!    romanized reading (or surface if reading is unknown "*"), capitalized.
//! 5. Join chunks with punctuation-aware single spacing.
//!
//! Output is an honest *draft*: morpheme-level segmentation can over-split
//! compounds and dropped rules #1/#2 leave loanwords (ドラキュラ -> "Dorakyura")
//! to a human. Overrides absorb the recurring special cases; the note flags the rest.

mod normalize;
mod overrides;
pub mod romaji;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;

use self::overrides::Overrides;
use super::{Script, TransliterationError, TransliterationOutput, Transliterator};

/// IPADIC `details` index of the katakana reading.
const READING_INDEX: usize = 7;
/// IPADIC `details` index of the part-of-speech.
const POS_INDEX: usize = 0;
/// IPADIC POS tag for particles.
const POS_PARTICLE: &str = "助詞";

/// Characters that never take a space *before* them when joining chunks.
const NO_SPACE_BEFORE: &str = ")]}>:;,.!?\"'-";
/// Characters that never take a space *after* them when joining chunks.
const NO_SPACE_AFTER: &str = "([{<\"'";

pub struct JapaneseTransliterator {
    tokenizer: Tokenizer,
    overrides: Overrides,
}

/// A segment of the normalized input.
enum Segment {
    /// An override or already-Latin run, emitted verbatim.
    Verbatim(String),
    /// A run of kana/kanji to tokenize and romanize.
    Japanese(String),
}

impl JapaneseTransliterator {
    /// Load the embedded IPADIC dictionary and build the tokenizer. This is
    /// relatively expensive, so construct once and share (see `TransliterationRegistry`).
    pub fn new() -> Result<Self, TransliterationError> {
        let dictionary = load_dictionary("embedded://ipadic")
            .map_err(|e| TransliterationError::Backend(format!("load ipadic: {e}")))?;
        let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
        let tokenizer = Tokenizer::new(segmenter);
        Ok(Self {
            tokenizer,
            overrides: Overrides::seed(),
        })
    }

    /// Romanize one run of Japanese text into capitalized, particle-aware words,
    /// applying tokenizer-aligned phrase overrides.
    fn romanize_run(&self, run: &str) -> Result<Vec<String>, TransliterationError> {
        let mut tokens = self
            .tokenizer
            .tokenize(run)
            .map_err(|e| TransliterationError::Backend(format!("tokenize: {e}")))?;

        // Collect per-token info up front (details() needs &mut self on the token).
        let mut info: Vec<(String, String, String)> = Vec::with_capacity(tokens.len());
        for token in tokens.iter_mut() {
            let surface = token.surface.to_string();
            let details = token.details();
            let pos = details
                .get(POS_INDEX)
                .map(|s| s.as_ref())
                .unwrap_or("")
                .to_string();
            let reading = details
                .get(READING_INDEX)
                .map(|s| s.as_ref())
                .unwrap_or("*")
                .to_string();
            info.push((surface, pos, reading));
        }
        let surfaces: Vec<String> = info.iter().map(|(s, _, _)| s.clone()).collect();

        let mut words = Vec::new();
        let mut i = 0;
        while i < info.len() {
            // Token-aligned phrase override (may span several tokens).
            if let Some((consumed, val)) = self.overrides.phrase_match(&surfaces, i) {
                words.push(val.to_string());
                i += consumed;
                continue;
            }

            let (surface, pos, reading) = &info[i];

            // Listed particles (rule #5): は/へ/を fixed, all lowercase.
            let is_particle = pos == POS_PARTICLE && is_listed_particle(surface);
            if is_particle {
                if let Some(p) = particle_romaji(surface) {
                    words.push(p.to_string());
                    i += 1;
                    continue;
                }
            }

            let romaji = if reading == "*" || reading.is_empty() {
                romaji::romanize_kana(surface)
            } else {
                romaji::romanize_kana(reading)
            };
            let romaji = romaji::expand_macrons(&romaji);

            words.push(if is_particle {
                romaji
            } else {
                capitalize(&romaji)
            });
            i += 1;
        }
        Ok(words)
    }
}

impl Transliterator for JapaneseTransliterator {
    fn script(&self) -> Script {
        Script::Japanese
    }

    fn transliterate(&self, input: &str) -> Result<TransliterationOutput, TransliterationError> {
        let normalized = normalize::normalize(input.trim());

        // Whole-title override wins outright.
        if let Some(pinned) = self.overrides.full_lookup(&normalized) {
            return Ok(TransliterationOutput {
                text: pinned.to_string(),
                script: Script::Japanese,
                notes: Vec::new(),
            });
        }

        // Pass A: segment into japanese vs passthrough (already-Latin) runs.
        // Phrase overrides are applied later, aligned to token boundaries inside
        // each Japanese run (see `romanize_run`).
        let mut segments: Vec<Segment> = Vec::new();
        let mut jp = String::new();
        let mut pass = String::new();
        for c in normalized.chars() {
            if is_japanese(c) {
                flush(&mut pass, &mut segments, Segment::Verbatim);
                jp.push(c);
            } else {
                flush(&mut jp, &mut segments, Segment::Japanese);
                pass.push(c);
            }
        }
        flush(&mut pass, &mut segments, Segment::Verbatim);
        flush(&mut jp, &mut segments, Segment::Japanese);

        // Pass B: render segments into chunks.
        let mut chunks: Vec<String> = Vec::new();
        for seg in segments {
            match seg {
                Segment::Verbatim(s) => chunks.push(s),
                Segment::Japanese(s) => {
                    chunks.extend(self.romanize_run(&s)?);
                }
            }
        }

        let text = capitalize_first_alpha(&join_chunks(&chunks));

        Ok(TransliterationOutput {
            text,
            script: Script::Japanese,
            notes: Vec::new(),
        })
    }
}

/// Move a non-empty accumulator into a new segment built by `make`.
fn flush(buf: &mut String, segments: &mut Vec<Segment>, make: fn(String) -> Segment) {
    if !buf.is_empty() {
        segments.push(make(std::mem::take(buf)));
    }
}

fn is_japanese(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        0x3040..=0x30FF   // hiragana + katakana (incl. chouonpu ー)
            | 0x31F0..=0x31FF // katakana phonetic extensions
            | 0x3400..=0x4DBF // CJK ext. A
            | 0x4E00..=0x9FFF // CJK unified ideographs
    )
}

/// The exact particle surfaces the framework enumerates for rule #5. We only
/// lowercase these (not every 助詞 token), so words like でも stay capitalized.
fn is_listed_particle(surface: &str) -> bool {
    matches!(
        surface,
        "は" | "が"
            | "を"
            | "へ"
            | "か"
            | "の"
            | "な"
            | "も"
            | "で"
            | "に"
            | "と"
            | "や"
            | "から"
            | "まで"
    )
}

/// Fixed particle romanizations where the reading differs from the spelling
/// (rule #5): は -> wa, へ -> e, を -> o. Other listed particles fall through to
/// normal (lowercased) romanization of their reading.
fn particle_romaji(surface: &str) -> Option<&'static str> {
    match surface {
        "は" => Some("wa"),
        "へ" => Some("e"),
        "を" => Some("o"),
        _ => None,
    }
}

/// Capitalize the first ASCII letter, leaving the rest untouched. A leading
/// non-letter (e.g. '-' on a suffix override) is preserved.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

/// Join chunks with punctuation-aware single spacing: a space is inserted
/// between two chunks unless either side already supplies a boundary (whitespace)
/// or the punctuation rules forbid it (e.g. no space before ':' or after '"').
fn join_chunks(chunks: &[String]) -> String {
    let mut out = String::new();
    for c in chunks {
        if c.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(c);
            continue;
        }
        let last = out.chars().last().unwrap();
        let first = c.chars().next().unwrap();
        let need_space = last != ' '
            && first != ' '
            && !NO_SPACE_BEFORE.contains(first)
            && !NO_SPACE_AFTER.contains(last);
        if need_space {
            out.push(' ');
        }
        out.push_str(c);
    }
    collapse_spaces(out.trim())
}

/// Uppercase the first alphabetic character of the final title, so a title that
/// begins with a lowercase connector override (the/of) or a lowercase loanword
/// still starts capitalized.
fn capitalize_first_alpha(s: &str) -> String {
    let mut done = false;
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if !done && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            done = true;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Collapse runs of spaces into a single space.
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

    fn t() -> JapaneseTransliterator {
        JapaneseTransliterator::new().expect("load ipadic")
    }

    fn tr(input: &str) -> String {
        t().transliterate(input).unwrap().text
    }

    #[test]
    fn romanizes_basic_titles() {
        assert_eq!(tr("どこでもいっしょ"), "Doko Demo Issho");
        assert_eq!(tr("恋愛"), "Ren'ai");
    }

    #[test]
    fn phrase_override_spans_tokens() {
        // 悪魔城 (over-split compound) + ドラキュラ (data-derived loanword) overrides
        // combine to the curated form.
        assert_eq!(tr("悪魔城ドラキュラ"), "Akumajou Dracula");
    }

    #[test]
    fn official_romanization_override() {
        assert_eq!(tr("ファミコン"), "Famicom");
        assert_eq!(tr("ラーメン"), "Ramen");
    }

    #[test]
    fn connector_overrides_and_first_letter_capitalized() {
        // オブ -> "of" (lowercase mid-title), other tokens romanized.
        assert_eq!(tr("テイルズ・オブ・ファンタジア"), "Tales of Fantajia");
        // Leading connector still yields a capitalized first letter.
        assert_eq!(tr("ザ・ワールド"), "The World");
    }

    #[test]
    fn normalization_quotes_and_middot() {
        assert_eq!(tr("『夢』"), "\"Yume\"");
        assert_eq!(tr("夢・島"), "Yume Shima");
    }

    #[test]
    fn normalization_brackets_and_separator() {
        assert_eq!(tr("【限定】夢"), "Gentei Yume");
        // Monospace space before a dash -> colon (subtitle style).
        assert_eq!(tr("夢　－島"), "Yume: Shima");
        // No monospace space before -> plain space.
        assert_eq!(tr("夢－島"), "Yume Shima");
    }

    #[test]
    fn passthrough_latin_and_digits() {
        // Fullwidth alnum folded; copied through verbatim.
        assert_eq!(tr("２Ｄ"), "2D");
    }

    #[test]
    fn capitalize_helper() {
        assert_eq!(capitalize("doko"), "Doko");
        assert_eq!(capitalize("-ban"), "-ban");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn join_punctuation_aware() {
        assert_eq!(join_chunks(&["Movie".into(), "-ban".into()]), "Movie-ban");
        assert_eq!(
            join_chunks(&["Akumajou".into(), ":".into(), "Densetsu".into()]),
            "Akumajou: Densetsu"
        );
        assert_eq!(
            join_chunks(&["\"".into(), "Yume".into(), "\"".into()]),
            "\"Yume\""
        );
    }

    #[test]
    #[ignore = "bench: cargo test --release ... bench_resources -- --ignored --nocapture"]
    fn bench_resources() {
        use std::time::Instant;
        let t0 = Instant::now();
        let t = JapaneseTransliterator::new().expect("load");
        let load_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let samples = [
            "悪魔城ドラキュラ",
            "ファイナルファンタジーⅦ",
            "テイルズ・オブ・ファンタジア",
            "ローライダー　～ラウンド・ザ・ワールド～",
            "どこでもいっしょ",
        ];
        let iters = 5000;
        let t1 = Instant::now();
        let mut sink = 0usize;
        for n in 0..iters {
            let s = samples[n % samples.len()];
            sink += t.transliterate(s).unwrap().text.len();
        }
        let per_call_us = t1.elapsed().as_secs_f64() * 1_000_000.0 / iters as f64;
        println!("\n== transliteration bench ==");
        println!("dictionary load: {load_ms:.0} ms (once, at startup)");
        println!("per call:        {per_call_us:.1} µs  (avg over {iters}, ~mixed titles)");
        println!(
            "throughput:      {:.0} calls/sec",
            1_000_000.0 / per_call_us
        );
        println!("(sink {sink})");
    }

    #[test]
    #[ignore = "bugcheck: TITLES_TSV=/tmp/jp_pairs.tsv cargo test ... bugcheck -- --ignored --nocapture"]
    fn bugcheck() {
        use rand::seq::SliceRandom;
        let path = std::env::var("TITLES_TSV").unwrap_or_default();
        if path.is_empty() {
            return;
        }
        let data = std::fs::read_to_string(&path).unwrap();
        let t = t();
        let is_kana = |c: char| matches!(c as u32, 0x3040..=0x30FF | 0x31F0..=0x31FF);
        let is_kanji = |c: char| matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF);

        let mut kana_leak = Vec::new(); // real bug: untranslated kana in output
        let mut kanji_leak = Vec::new(); // missing dictionary reading
        let mut all: Vec<(String, String)> = Vec::new();
        let mut n = 0u64;
        for line in data.lines() {
            let Some((fg, ti)) = line.split_once('\t') else {
                continue;
            };
            let Ok(out) = t.transliterate(fg) else {
                continue;
            };
            n += 1;
            let txt = out.text;
            if txt.chars().any(|c| c != 'ー' && is_kana(c)) {
                kana_leak.push((fg.to_string(), txt.clone()));
            } else if txt.chars().any(is_kanji) {
                kanji_leak.push((fg.to_string(), txt.clone()));
            }
            all.push((fg.to_string(), txt));
            let _ = ti;
        }

        println!("\n== bug scan over {n} entries ==");
        println!("kana leaks (BUG):     {}", kana_leak.len());
        println!("kanji leaks (no reading): {}", kanji_leak.len());

        println!("\n-- sample kana leaks (if any) --");
        for (fg, txt) in kana_leak.iter().take(15) {
            println!("  {fg}  ->  {txt}");
        }
        println!("\n-- sample kanji leaks --");
        for (fg, txt) in kanji_leak.iter().take(15) {
            println!("  {fg}  ->  {txt}");
        }

        println!("\n-- 50 random samples (eyeball) --");
        let mut rng = rand::thread_rng();
        for (fg, txt) in all.choose_multiple(&mut rng, 50) {
            let flag = if txt
                .chars()
                .any(|c| c != 'ー' && (is_kana(c) || is_kanji(c)))
            {
                "   <== LEFTOVER"
            } else {
                ""
            };
            println!("  {fg}  ->  {txt}{flag}");
        }
    }

    #[test]
    #[ignore = "sample: TITLES_TSV=/tmp/jp_pairs.tsv cargo test ... sample_mismatches -- --ignored --nocapture"]
    fn sample_mismatches() {
        use rand::seq::SliceRandom;
        let path = std::env::var("TITLES_TSV").unwrap_or_default();
        if path.is_empty() {
            return;
        }
        let data = std::fs::read_to_string(&path).unwrap();
        let t = t();
        let mut miss: Vec<(String, String, String)> = Vec::new(); // (jp, engine, curated)
        for line in data.lines() {
            let Some((fg, ti)) = line.split_once('\t') else {
                continue;
            };
            let Ok(out) = t.transliterate(fg) else {
                continue;
            };
            if out.text.to_lowercase() != ti.to_lowercase() {
                miss.push((fg.to_string(), out.text.clone(), ti.to_string()));
            }
        }
        let mut rng = rand::thread_rng();
        let picks: Vec<_> = miss.choose_multiple(&mut rng, 10).cloned().collect();
        for (jp, eng, cur) in picks {
            println!("Redump.org Entry title: {cur}");
            println!("Transliterate function title: {eng}");
            println!("Japanese title: {jp}");
            println!();
        }
    }

    #[test]
    #[ignore = "mining: TITLES_TSV=/tmp/jp_pairs.tsv cargo test ... mine_compounds -- --ignored --nocapture"]
    fn mine_compounds() {
        let path = std::env::var("TITLES_TSV").unwrap_or_default();
        if path.is_empty() {
            return;
        }
        let data = std::fs::read_to_string(&path).unwrap();
        let t = t();
        let all_kanji = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF))
        };
        // surface-pair -> (joined romaji, count of entries where joining matches curated)
        let mut tally: std::collections::HashMap<String, (String, u32)> =
            std::collections::HashMap::new();
        for line in data.lines() {
            let Some((fg, ti)) = line.split_once('\t') else {
                continue;
            };
            let ti_l = ti.to_lowercase();
            let mut tokens = t.tokenizer.tokenize(fg).unwrap();
            let info: Vec<(String, String)> = tokens
                .iter_mut()
                .map(|tok| {
                    let s = tok.surface.to_string();
                    let r = tok
                        .details()
                        .get(READING_INDEX)
                        .map(|x| x.as_ref())
                        .unwrap_or("*")
                        .to_string();
                    (s, r)
                })
                .collect();
            for w in info.windows(2) {
                let (s1, r1) = &w[0];
                let (s2, r2) = &w[1];
                if !all_kanji(s1) || !all_kanji(s2) || r1 == "*" || r2 == "*" {
                    continue;
                }
                let a = romaji::romanize_kana(r1);
                let b = romaji::romanize_kana(r2);
                let joined = format!("{a}{b}").to_lowercase();
                let spaced = format!("{a} {b}").to_lowercase();
                if joined.len() < 5 {
                    continue;
                }
                if ti_l.contains(&joined) && !ti_l.contains(&spaced) {
                    let key = format!("{s1}{s2}");
                    let val = capitalize(&format!("{a}{b}"));
                    let e = tally.entry(key).or_insert((val, 0));
                    e.1 += 1;
                }
            }
        }
        let mut v: Vec<_> = tally.into_iter().collect();
        v.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
        println!("\n== top over-split compounds (surface -> joined, count) ==");
        for (k, (val, c)) in v.iter().take(40) {
            println!("  {c:4}  {k}  ->  {val}");
        }
    }

    #[test]
    #[ignore = "analysis: TITLES_TSV=/tmp/jp_pairs.tsv cargo test ... -- --ignored --nocapture"]
    fn analyze_dataset() {
        let path = std::env::var("TITLES_TSV").unwrap_or_default();
        if path.is_empty() {
            eprintln!("set TITLES_TSV to run");
            return;
        }
        let data = std::fs::read_to_string(&path).unwrap();
        let t = t();
        let (mut n, mut exact, mut ci_exact) = (0u64, 0u64, 0u64);
        // Subset with NO katakana loanwords -- titles the engine is fully
        // responsible for (case-insensitive match).
        let (mut n_nk, mut ci_nk, mut sp_nk) = (0u64, 0u64, 0u64);
        let mut sp_all = 0u64; // space+case-insensitive match (romanization correct, spacing aside)
        let has_katakana = |s: &str| s.chars().any(|c| matches!(c as u32, 0x30A1..=0x30FA));
        let squash = |s: &str| s.to_lowercase().replace([' ', '-', ':'], "");
        let mut mismatches: Vec<(String, String, String)> = Vec::new();
        for line in data.lines() {
            let Some((fg, ti)) = line.split_once('\t') else {
                continue;
            };
            let Ok(out) = t.transliterate(fg) else {
                continue;
            };
            n += 1;
            let ci = out.text.to_lowercase() == ti.to_lowercase();
            if out.text == ti {
                exact += 1;
            } else if ci {
                ci_exact += 1;
            } else if mismatches.len() < 25 {
                mismatches.push((fg.to_string(), out.text.clone(), ti.to_string()));
            }
            let sp = squash(&out.text) == squash(ti);
            if sp {
                sp_all += 1;
            }
            if !has_katakana(fg) {
                n_nk += 1;
                if ci {
                    ci_nk += 1;
                }
                if sp {
                    sp_nk += 1;
                }
            }
        }
        println!("\n== {n} entries ==");
        println!(
            "exact match:           {exact}  ({:.1}%)",
            100.0 * exact as f64 / n as f64
        );
        println!(
            "case-insensitive match:{ci_exact}  ({:.1}%)",
            100.0 * ci_exact as f64 / n as f64
        );
        println!(
            "combined:              {}  ({:.1}%)",
            exact + ci_exact,
            100.0 * (exact + ci_exact) as f64 / n as f64
        );
        println!(
            "no-katakana subset:    {ci_nk}/{n_nk}  ({:.1}%)  [kanji/kana-only titles]",
            100.0 * ci_nk as f64 / n_nk as f64
        );
        println!(
            "romanization-correct (ignoring spacing/case): all {:.1}%, no-katakana {:.1}%",
            100.0 * sp_all as f64 / n as f64,
            100.0 * sp_nk as f64 / n_nk as f64
        );
        println!("\n-- sample mismatches (foreign | engine | curated) --");
        for (fg, eng, ti) in mismatches.iter().take(20) {
            println!("  {fg}\n    engine : {eng}\n    curated: {ti}");
        }
    }

    #[test]
    #[ignore = "demo: run with --ignored --nocapture to see real output"]
    fn demo_pdf_examples() {
        let t = t();
        for s in [
            "どこでもいっしょ",
            "悪魔城ドラキュラ",
            "恋愛",
            "列車",
            "四つ",
            "抹茶",
            "千葉県",
            "ガーン",
            "ファミコン",
            "ラーメン",
            "『夢』・島",
            "ＲＰＧ ２",
            "悪魔城ドラキュラ　Ｘ～月下の夜想曲～　オリジナル・ゲーム・サントラ",
            "コール オブ デューティー:ファイネスト アワー",
        ] {
            let out = t.transliterate(s).unwrap();
            println!("{s}  ->  {}", out.text);
        }
    }
}
