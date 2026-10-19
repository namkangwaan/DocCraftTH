//! Literal search in normalized page text, retaining the original glyph owners.
use super::{PageText, fold_case};
use std::ops::Range;

/// A comparison character and the first and last glyph it came from (two when glyphs compose).
type Entry = (char, usize, usize);

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let fold = |s: &str| if sensitive { s.to_owned() } else { fold_case(s) };
    let needle = fold(query).split_whitespace().collect::<Vec<_>>().join(" ").chars().map(|c| (c, 0, 0)).collect();
    let needle: Vec<char> = normalise(needle).into_iter().map(|e| e.0).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut chars: Vec<Entry> = Vec::new();
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || line_break;
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i, i));
        }
        for c in fold(&glyph.text).chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, i, i));
            }
        }
    }
    let chars = normalise(chars);
    if needle.len() > chars.len() {
        return Vec::new();
    }
    // KMP reuses a matched prefix instead of comparing a long query again at every position.
    // The table is bounded by the page's already materialized character count.
    let mut prefix = vec![0; needle.len()];
    let mut matched = 0;
    for (i, &c) in needle.iter().enumerate().skip(1) {
        matched = advance(&needle, &prefix, matched, c);
        if let Some(slot) = prefix.get_mut(i) {
            *slot = matched;
        }
    }
    matched = 0;
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.0.is_alphanumeric());
    let mut hits = Vec::new();
    for (i, &(c, _, owner)) in chars.iter().enumerate() {
        matched = advance(&needle, &prefix, matched, c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        let bounded = !whole_words || (!word(start.checked_sub(1)) && !word(Some(end)));
        if bounded {
            if let Some((_, first, _)) = chars.get(start)
                && let Some(last) = owner.checked_add(1)
            {
                hits.push(*first..last);
                if hits.len() == limit {
                    break;
                }
            }
            // Only accepted hits consume their full span: literal search is non-overlapping.
            matched = 0;
        } else {
            // A word-boundary rejection must still allow an overlapping later candidate.
            matched = prefix.last().copied().unwrap_or_default();
        }
    }
    hits
}

/// Spellings that render alike compare alike, in the query and the page, in both case modes (this
/// is not a case distinction): NIKHAHIT + SARA AA (`ํา`, how Word exports `ำ`) is SARA AM (`ำ`).
/// The composed entry keeps both glyphs, so a hit still covers the SARA AA glyph.
fn normalise(chars: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(chars.len());
    for e in chars {
        if e.0 == '\u{0E32}'
            && let Some(last) = out.last_mut()
            && last.0 == '\u{0E4D}'
        {
            *last = ('\u{0E33}', last.1, e.2);
        } else {
            out.push(e);
        }
    }
    out
}

/// Extend a prefix, falling back through strictly shorter proper prefixes on a mismatch.
fn advance(needle: &[char], prefix: &[usize], mut matched: usize, c: char) -> usize {
    while matched > 0 && needle.get(matched) != Some(&c) {
        matched = prefix.get(matched - 1).copied().unwrap_or_default();
    }
    if needle.get(matched) == Some(&c) { matched.saturating_add(1) } else { 0 }
}
