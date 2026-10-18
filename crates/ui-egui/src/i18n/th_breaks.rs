//! Thai is written without spaces between words, and egui wraps only at spaces (and between any
//! two characters as a last resort), so a Thai label could wrap mid-word or before a tone mark.
//! The `th` catalog's translations carry U+200B ZERO WIDTH SPACE at each word boundary, which the
//! vendored epaint treats as a break opportunity and draws as nothing. Text that leaves the
//! screen (into a PDF, a search, a control-channel match) goes through [`without_soft_breaks`].

use std::borrow::Cow;

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
