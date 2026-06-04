//! Curated overrides for Japanese transliteration.
//!
//! The general engine (lindera readings + `romaji` rules) handles the bulk of
//! titles phonetically. Overrides capture cases that are *judgment*, not
//! mechanics, and so cannot be derived algorithmically.
//!
//! ## Matching model: tokenizer-aligned
//!
//! Phrase overrides match against lindera **token boundaries**, not raw
//! substrings. A key is the concatenation of one or more consecutive token
//! *surfaces*; it only fires when it lines up with whole tokens. This avoids the
//! substring trap -- an override for スター ("Star") never matches inside the
//! single token モンスター ("Monster") -- while still letting a multi-token
//! compound like 悪魔城 (tokenized 悪魔 + 城) be pinned to "Akumajou".
//!
//! ## Granularities
//! - `full`   : exact whole-input (normalized) match -> exact output. Highest priority.
//! - `phrase` : a token-surface sequence -> a fixed output fragment, matched
//!              greedily (longest token run first).
//!
//! A leading '-' on a phrase output attaches it to the previous word without a
//! space (e.g. a "-ban" suffix -> "Movie-ban").
//!
//! Only **unambiguous** entries belong here. Ambiguous loanwords (ロード =
//! Lord/Road/Load, フォー = Four/for) are deliberately left to mechanical
//! romanization + manual edit. Titles whose official English is printed on the
//! item (e.g. バイオハザード) are also left manual.

use std::collections::HashMap;

/// Maximum number of consecutive tokens a phrase override may span.
const MAX_PHRASE_TOKENS: usize = 8;

pub struct Overrides {
    full: HashMap<String, String>,
    phrase: HashMap<String, String>,
}

impl Overrides {
    /// Build the seed override set.
    pub fn seed() -> Self {
        let mut full: HashMap<String, String> = HashMap::new();
        let mut phrase: HashMap<String, String> = HashMap::new();

        // --- Whole-title pins (normalized input -> Main Title) ---
        // (none yet; add titles that need exact, hand-verified output)
        let _ = &mut full;

        for (k, v) in PHRASE_SEED {
            phrase.insert(k.to_string(), v.to_string());
        }

        Self { full, phrase }
    }

    /// Exact whole-input override (input should already be normalized).
    pub fn full_lookup(&self, input: &str) -> Option<&str> {
        self.full.get(input.trim()).map(|s| s.as_str())
    }

    /// Longest token-aligned phrase override starting at `start` in `surfaces`.
    /// Returns the number of tokens consumed and the replacement output.
    pub fn phrase_match(&self, surfaces: &[String], start: usize) -> Option<(usize, &str)> {
        let max = MAX_PHRASE_TOKENS.min(surfaces.len() - start);
        for k in (1..=max).rev() {
            let candidate: String = surfaces[start..start + k].concat();
            if let Some(v) = self.phrase.get(&candidate) {
                return Some((k, v.as_str()));
            }
        }
        None
    }
}

/// Seed phrase overrides (token-surface -> output). Grouped by theme; grown from
/// real dataset analysis. Keep entries unambiguous.
const PHRASE_SEED: &[(&str, &str)] = &[
    // --- Official romanizations that deviate from strict Hepburn ---
    ("ファミコン", "Famicom"),
    ("ラーメン", "Ramen"),
    ("ファミ通", "Famitsu"),
    // --- Multi-token kanji compounds that segmentation over-splits ---
    ("悪魔城", "Akumajou"),
    // --- Common loanword nouns (frequency in the 31k-entry dataset in
    //     parentheses; all verified unambiguous against real titles) ---
    ("シリーズ", "Series"),        // 709
    ("サウンドトラック", "Soundtrack"), // 527
    ("サントラ", "Soundtrack"),     // 45 (abbreviation of サウンドトラック)
    ("オリジナル", "Original"),     // 473
    ("ゲーム", "Game"),            // 335
    ("コレクション", "Collection"), // 231
    ("サウンド", "Sound"),         // 223
    ("スペシャル", "Special"),      // 204
    ("ミュージック", "Music"),      // 171
    ("ワールド", "World"),         // 162
    ("ポータブル", "Portable"),     // 146
    ("スーパー", "Super"),         // 123
    ("キング", "King"),            // 103
    ("アーケード", "Arcade"),       // 96
    ("テイルズ", "Tales"),         // 94
    ("オンライン", "Online"),       // 87
    ("デジタル", "Digital"),        // 87
    ("ウォーズ", "Wars"),          // 74
    ("スター", "Star"),            // 72
    ("コール", "Call"),            // 66
    ("デッド", "Dead"),            // 64
    ("ベスト", "Best"),            // 37
    // --- Structural connectors (lowercase; the final title's first letter is
    //     re-capitalized downstream so a leading connector still looks right) ---
    ("オブ", "of"),               // 766
    ("ザ", "the"),                // 818
    ("アンド", "and"),             // 39
    // --- Platforms / companies ---
    ("プレイステーション", "PlayStation"), // 216
    ("マイクロソフト", "Microsoft"),    // 66
    // --- Kanji compounds the tokenizer over-splits (mined + verified against the
    //     dataset: count = curated titles using the joined form). These are joined,
    //     never hyphenated -- matching the framework's non-hyphenated suffix cases
    //     (限定版 -> Genteiban, 恋姫 -> Koihime). ---
    ("限定版", "Genteiban"),     // 112
    ("錬金術士", "Renkinjutsushi"), // 78
    ("大戦略", "Daisenryaku"),   // 58
    ("大作戦", "Daisakusen"),    // 57
    ("大冒険", "Daibouken"),     // 52
    ("必勝法", "Hisshouhou"),    // 52
    ("王子様", "Oujisama"),      // 47
    ("完全版", "Kanzenban"),     // 46
    ("体験版", "Taikenban"),     // 43
    ("大航海", "Daikoukai"),     // 34
    ("奇譚", "Kitan"),           // 33
    ("将伝", "Shouden"),         // 33
    ("猛将", "Moushou"),         // 33
    ("決定版", "Ketteiban"),     // 29
    ("大全集", "Daizenshuu"),    // 28
    ("山佐", "Yamasa"),          // 27
    ("鬼武者", "Onimusha"),      // 23
    ("異聞録", "Ibunroku"),      // 23
    ("事件簿", "Jikenbo"),       // 22
    ("錬金術師", "Renkinjutsushi"), // 21
    ("疾風伝", "Shippuuden"),    // 20
    ("魔人", "Majin"),           // 20
    ("名探偵", "Meitantei"),     // 19
    ("神伝", "Shinden"),         // 19
    ("機神", "Kishin"),          // 18
    ("第二", "Daini"),           // 16
    ("恋姫", "Koihime"),         // 16
    ("大乱闘", "Dairantou"),     // 16
    ("戦極", "Sengoku"),         // 16
    ("豪華版", "Goukaban"),      // 15
    ("原画集", "Gengashuu"),     // 15
    ("総選挙", "Sousenkyo"),     // 14
    ("虫姫", "Mushihime"),       // 13
    ("魔装", "Masou"),           // 13
    // --- Recurring loanwords / franchise names. Both the katakana key and the
    //     English value are DATA-DERIVED: mined by statistical association with the
    //     curated English titles and hand-verified (count in parens). Spurious
    //     co-occurrences, fragments, and titles with varying printed English
    //     (バイオハザード) were excluded. ---
    ("ファイナルファンタジー", "Final Fantasy"), // 223
    ("ドラマ", "Drama"),         // 254
    ("ガンダム", "Gundam"),       // 171
    ("サクラ", "Sakura"),         // 135
    ("プロモーション", "Promotion"), // 133
    ("イース", "Ys"),             // 110
    ("アトリエ", "Atelier"),       // 97
    ("メモリアル", "Memorial"),    // 88
    ("パワフルプロ", "Powerful Pro"), // 86
    ("ウイニングポスト", "Winning Post"), // 83
    ("ウイニングイレブン", "Winning Eleven"), // 82
    ("プリンセス", "Princess"),    // 79
    ("ドラキュラ", "Dracula"),     // 79
    ("ファンタシースターオンライン", "Phantasy Star Online"), // 66
    ("ソニック", "Sonic"),         // 66
    ("ゼルダ", "Zelda"),          // 65
    ("ロックマン", "Rockman"),     // 64
    ("テニス", "Tennis"),         // 66
    ("ドラゴンクエスト", "Dragon Quest"), // 71
    ("アルティメット", "Ultimate"), // 71
    ("ディアボリックラヴァーズ", "Diabolik Lovers"), // 70
    ("クイズ", "Quiz"),           // 67
    ("デモ", "Demo"),             // 70
    ("ダンジョン", "Dungeon"),     // 56
    ("アンジェリーク", "Angelique"), // 58
    ("エヴァンゲリオン", "Evangelion"), // 58
    ("パチスロ", "Pachi-Slot"),    // 154
    ("パワーアップキット", "Power-Up Kit"), // 87
];

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaces(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn matches_single_token() {
        let o = Overrides::seed();
        let s = surfaces(&["ファミコン", "ソフト"]);
        assert_eq!(o.phrase_match(&s, 0), Some((1, "Famicom")));
    }

    #[test]
    fn matches_multi_token_compound() {
        let o = Overrides::seed();
        // 悪魔城 tokenized as 悪魔 + 城 -> 2 tokens consumed.
        let s = surfaces(&["悪魔", "城", "ドラキュラ"]);
        assert_eq!(o.phrase_match(&s, 0), Some((2, "Akumajou")));
    }

    #[test]
    fn does_not_match_inside_a_token() {
        let o = Overrides::seed();
        // A hypothetical スター override must not fire inside モンスター; here we
        // confirm a non-seeded single token simply misses.
        let s = surfaces(&["モンスター"]);
        assert_eq!(o.phrase_match(&s, 0), None);
    }
}
