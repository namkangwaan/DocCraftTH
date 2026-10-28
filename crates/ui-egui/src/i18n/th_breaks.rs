//! Thai is written without spaces between words, and egui wraps only at spaces (and between any
//! two characters as a last resort), so a Thai label could wrap mid-word or before a tone mark.
//! The `th` catalog's translations carry U+200B ZERO WIDTH SPACE at each word boundary, which
//! [`ThaiLineBreaker`] treats as a break opportunity (epaint draws it as nothing). Text that leaves the
//! screen (into a PDF, a search, a control-channel match) goes through [`without_soft_breaks`].

use std::borrow::Cow;

use egui::epaint::text::{LineBreak, LineBreaker, SimpleLineBreaker};
use pdfcraft_fonts::thai::{is_thai, may_break};

const SOFT_BREAK: char = '\u{200B}';

/// `text` with U+200B wherever a line may end inside Thai text. Borrows when there is none.
pub(crate) fn soft_breaks(text: &str) -> Cow<'_, str> {
    let offsets = pdfcraft_fonts::thai::break_offsets(text);
    if offsets.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len().saturating_add(offsets.len().saturating_mul(SOFT_BREAK.len_utf8())));
    let mut start = 0;
    for at in offsets {
        out.push_str(text.get(start..at).unwrap_or_default());
        out.push(SOFT_BREAK);
        start = at;
    }
    out.push_str(text.get(start..).unwrap_or_default());
    Cow::Owned(out)
}

/// `text` without the U+200B [`soft_breaks`] added. Borrows when there is none.
pub(crate) fn without_soft_breaks(text: &str) -> Cow<'_, str> {
    if text.contains(SOFT_BREAK) { Cow::Owned(text.replace(SOFT_BREAK, "")) } else { Cow::Borrowed(text) }
}

/// The penalty of a break between two syllables of one Thai word: a row ends there only when it
/// holds no word boundary (a space or U+200B, penalty 0) to end at instead.
const SYLLABLE_PENALTY: u8 = 3;

/// Where a row of text may end: epaint's [`SimpleLineBreaker`] rules, plus after U+200B, plus
/// (as a last resort) between Thai syllables. A row therefore never starts with a mark or a
/// following vowel and never ends with a leading vowel ([`may_break`]), where epaint's own
/// last resort, any grapheme boundary, would split them.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThaiLineBreaker;

impl LineBreaker for ThaiLineBreaker {
    fn line_breaks(&self, text: &str, language: Option<&str>, out: &mut Vec<LineBreak>) {
        SimpleLineBreaker.line_breaks(text, language, out);
        let mut chars = text.char_indices().peekable();
        while let Some((_, before)) = chars.next() {
            let Some(&(at, after)) = chars.peek() else { break };
            if before == SOFT_BREAK {
                out.push(LineBreak::soft(at));
            } else if is_thai(before) && is_thai(after) && may_break(before, after) {
                out.push(LineBreak::soft_with_penalty(at, SYLLABLE_PENALTY));
            }
        }
    }
}
