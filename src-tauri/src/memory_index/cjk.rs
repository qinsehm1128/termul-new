//! CJK bigram expansion, so full-text search works in the language this
//! project's transcripts are actually written in.
//!
//! ## The problem this exists to solve
//!
//! FTS5's `unicode61` tokenizer decides token boundaries from Unicode general
//! categories: everything in `L*` or `N*` is a token character, everything else
//! is a separator. CJK ideographs are `Lo`, so an unbroken run of Chinese is
//! **one token**. Measured against a live SQLite:
//!
//! ```text
//! document: 重构了内存索引的存储层
//!   MATCH "内存"                 -> 0 rows
//!   MATCH "内存索引"              -> 0 rows
//!   MATCH "重构了内存索引的存储层"  -> 1 row     <- only an exact whole-run match
//! ```
//!
//! `tokenize = 'trigram'` fixes queries of three characters or more but leaves
//! two-character queries at zero rows, and two-character words are the common
//! case in Chinese (`会话`, `内存`, `索引`, `存储`). So the tokenizer stays
//! `unicode61` and the text is expanded instead.
//!
//! ## The expansion
//!
//! Every maximal run of CJK characters becomes its overlapping bigrams, emitted
//! as separate space-separated tokens:
//!
//! ```text
//! 内存索引  ->  内存 存索 索引
//! ```
//!
//! A query is expanded the same way, so `内存` matches the `内存` bigram and
//! `内存索引` matches all three with FTS5's implicit AND. A single-character run
//! is emitted as itself, and a single-character *query* becomes a prefix match
//! (`"内"*`), which finds every bigram that starts with it.
//!
//! ## Why a separate column
//!
//! The expansion goes in its own `cjk` column rather than replacing the text.
//! Latin content — which is most of the corpus, being tool output, paths and
//! code — expands to the empty string and costs nothing, while `f.text` stays
//! the exact text a hit displays. Only records that actually contain CJK pay,
//! and they pay about 2.3x on their CJK portion alone.

/// Is this character part of a CJK run?
///
/// Covers the ideograph blocks plus kana and Hangul syllables. All of them share
/// the property that makes this module necessary: `unicode61` treats them as
/// token characters, and the scripts do not separate words with spaces.
#[must_use]
pub fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3040..=0x30FF      // Hiragana + Katakana
        | 0x3400..=0x4DBF    // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF    // CJK Unified Ideographs
        | 0xAC00..=0xD7AF    // Hangul syllables
        | 0xF900..=0xFAFF    // CJK Compatibility Ideographs
        | 0x20000..=0x2FA1F  // Extensions B-F + Compatibility Supplement
    )
}

/// Does this text contain anything the expansion would produce?
#[must_use]
pub fn has_cjk(text: &str) -> bool {
    text.chars().any(is_cjk)
}

/// Expand every CJK run in `text` into space-separated overlapping bigrams.
///
/// Non-CJK content is dropped entirely: it is already searchable through the
/// `text` column, and repeating it here would double the index for no gain.
/// Returns an empty string for text with no CJK, which is the common case.
#[must_use]
pub fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut run: Vec<char> = Vec::new();

    for character in text.chars() {
        if is_cjk(character) {
            run.push(character);
        } else if !run.is_empty() {
            push_run(&mut out, &run);
            run.clear();
        }
    }
    if !run.is_empty() {
        push_run(&mut out, &run);
    }
    out
}

fn push_run(out: &mut String, run: &[char]) {
    match run {
        [] => {}
        [single] => {
            push_token(out, &single.to_string());
        }
        _ => {
            for pair in run.windows(2) {
                push_token(out, &pair.iter().collect::<String>());
            }
        }
    }
}

fn push_token(out: &mut String, token: &str) {
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(token);
}

/// The FTS5 terms a CJK run in a *query* should match against.
///
/// Mirrors [`expand`] with one deliberate difference: a one-character run
/// becomes a prefix term rather than a literal, so `索` finds `索引`, `搜索`'s
/// second bigram and anything else whose bigram starts with it. Without that a
/// single character would only match a character standing entirely alone.
#[must_use]
pub fn query_terms(run: &str) -> Vec<String> {
    let characters: Vec<char> = run.chars().collect();
    match characters.as_slice() {
        [] => Vec::new(),
        [single] => vec![format!("\"{single}\"*")],
        _ => characters
            .windows(2)
            .map(|pair| format!("\"{}\"", pair.iter().collect::<String>()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_becomes_its_overlapping_bigrams() {
        assert_eq!(expand("内存索引"), "内存 存索 索引");
    }

    #[test]
    fn latin_text_expands_to_nothing_so_it_costs_no_storage() {
        assert_eq!(expand("the quick brown fox --no-verify ENOENT"), "");
        assert!(!has_cjk("plain ascii"));
    }

    #[test]
    fn runs_are_split_at_every_non_cjk_character() {
        // `把 claude 的历史会话` -> two runs, not one bigram spanning `把` and `的`.
        assert_eq!(expand("把 claude 的历史会话"), "把 的历 历史 史会 会话");
    }

    #[test]
    fn a_lone_character_is_kept_as_itself() {
        assert_eq!(expand("a 好 b"), "好");
    }

    #[test]
    fn kana_and_hangul_are_expanded_too() {
        assert_eq!(expand("こんにちは"), "こん んに にち ちは");
        assert_eq!(expand("안녕하세요"), "안녕 녕하 하세 세요");
    }

    #[test]
    fn a_single_character_query_becomes_a_prefix_term() {
        assert_eq!(query_terms("索"), vec!["\"索\"*".to_string()]);
    }

    #[test]
    fn a_multi_character_query_becomes_every_bigram() {
        assert_eq!(
            query_terms("历史会话"),
            vec![
                "\"历史\"".to_string(),
                "\"史会\"".to_string(),
                "\"会话\"".to_string(),
            ]
        );
    }

    #[test]
    fn query_terms_of_a_two_character_word_match_what_expand_emits() {
        // The load-bearing property: whatever `expand` writes, `query_terms`
        // must be able to name. If these two ever drift, search silently
        // returns nothing.
        let stored = expand("会话记录");
        for term in query_terms("会话") {
            let bare = term.trim_matches('"');
            assert!(
                stored.split_whitespace().any(|token| token == bare),
                "query term {term} is absent from stored expansion {stored:?}"
            );
        }
    }

    #[test]
    fn surrogate_range_ideographs_are_recognised() {
        // U+20000 is outside the BMP; a `char as u32` comparison has to reach it.
        assert!(is_cjk('\u{20000}'));
        assert!(!is_cjk('a'));
        assert!(!is_cjk('！')); // fullwidth punctuation is a separator, not a token
    }
}
