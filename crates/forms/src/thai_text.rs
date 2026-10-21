//! Thai text in form text fields: layout, wrapping and drawing as glyph outlines.
//!
//! A field's appearance is built from a `&Document`, so it cannot add the indirect objects a
//! Type 3 font needs. The Thai part of a value is instead drawn as filled paths from the
//! craft-fonts Thai face (shaped with [`shape_thai`], so vowels and tone marks sit on their
//! letter), and each line is wrapped in `/ActualText` so copying gives the typed text. The rest of
//! the line (Latin, digits, spaces) stays ordinary text in the field's own font.
//!
//! [`Thai::new`] says whether a value can be drawn this way: it needs the face, a Thai block that
//! the face covers, and at most [`MAX_CHARS`] characters. Otherwise the caller keeps its `?` path.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pdfcraft_fonts::{GlyphOutline, ShapedCluster, ShapedScript, shape_thai, shaped_glyph, thai, thai_has};

use crate::appearance::fmt;

/// The most characters laid out as outlines; a longer value keeps the `?` path (a thousand
/// clusters is a few hundred thousand path points at most).
pub(crate) const MAX_CHARS: usize = 1000;
/// Line step of multiline Thai, in ems: marks stacked above a letter and vowels below it need
/// more room than Helvetica's 1.15.
pub(crate) const LINE_STEP: f64 = 1.35;

/// Width of a non-Thai span at a size, from the field's own font.
pub(crate) type LatinWidth<'a> = &'a dyn Fn(&str, f64) -> f64;

/// A piece of a line: the Thai clusters (advance in ems), or one non-Thai character.
struct Atom {
    text: String,
    thai_em: Option<f64>,
    /// Byte offset in the paragraph just after this atom.
    end: usize,
}

/// A value that can be drawn with the Thai face, and the shaping it needs.
pub(crate) struct Thai {
    shaped: RefCell<HashMap<String, Option<Rc<Vec<ShapedCluster>>>>>,
    outlines: RefCell<HashMap<u32, Option<Rc<GlyphOutline>>>>,
}

/// `text` cut into runs that are all Thai or all not.
fn spans(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut kind: Option<bool> = None;
    for (at, c) in text.char_indices() {
        let is = thai::is_thai(c);
        if kind.is_some_and(|k| k != is) {
            out.extend(text.get(start..at).map(|s| (kind.unwrap_or(false), s)));
            start = at;
        }
        kind = Some(is);
    }
    out.extend(text.get(start..).filter(|s| !s.is_empty()).map(|s| (kind.unwrap_or(false), s)));
    out
}

/// `text` without its Thai block characters: what the field's own font has to hold.
pub(crate) fn without_thai(text: &str) -> String {
    text.chars().filter(|c| !thai::is_thai(*c)).collect()
}

impl Thai {
    /// The layout for `text`, when it has Thai in it and every Thai character can be drawn.
    pub(crate) fn new(text: &str) -> Option<Thai> {
        if text.chars().count() > MAX_CHARS || !text.chars().any(thai::is_thai) {
            return None;
        }
        if text.chars().any(|c| thai::is_thai(c) && !thai_has(c)) {
            return None;
        }
        let me = Thai { shaped: RefCell::new(HashMap::new()), outlines: RefCell::new(HashMap::new()) };
        for (is_thai, span) in spans(text) {
            if is_thai && me.shape(span).is_none() {
                return None;
            }
        }
        Some(me)
    }

    fn shape(&self, span: &str) -> Option<Rc<Vec<ShapedCluster>>> {
        if let Some(found) = self.shaped.borrow().get(span) {
            return found.clone();
        }
        let shaped = shape_thai(span).ok().map(Rc::new);
        self.shaped.borrow_mut().insert(span.to_string(), shaped.clone());
        shaped
    }

    fn outline(&self, id: u32) -> Option<Rc<GlyphOutline>> {
        if let Some(found) = self.outlines.borrow().get(&id) {
            return found.clone();
        }
        let outline = shaped_glyph(ShapedScript::Thai, id).ok().map(Rc::new);
        self.outlines.borrow_mut().insert(id, outline.clone());
        outline
    }

    /// The width of one line at `size`.
    pub(crate) fn width(&self, text: &str, size: f64, latin: LatinWidth) -> f64 {
        spans(text)
            .into_iter()
            .map(|(is_thai, span)| {
                if is_thai {
                    // `new` shaped every Thai run of the value; a piece of one shapes the same way.
                    self.shape(span).map_or(0.0, |c| c.iter().map(|c| c.advance).sum::<f64>() * size)
                } else {
                    latin(span, size)
                }
            })
            .sum()
    }

    /// The atoms of a paragraph: Thai clusters (never split) and single other characters.
    fn atoms(&self, para: &str) -> Vec<Atom> {
        let mut out = Vec::new();
        let mut at = 0usize;
        for (is_thai, span) in spans(para) {
            if let (true, Some(clusters)) = (is_thai, self.shape(span)) {
                for c in clusters.iter() {
                    at += c.text.len();
                    out.push(Atom { text: c.text.clone(), thai_em: Some(c.advance), end: at });
                }
            } else {
                for ch in span.chars() {
                    at += ch.len_utf8();
                    out.push(Atom { text: ch.to_string(), thai_em: None, end: at });
                }
            }
        }
        out
    }

    /// `text` wrapped to `width` at `size`: between words (spaces and the Thai word model), and
    /// inside a word wider than the row only between clusters, so no mark leaves its letter.
    /// A line's trailing spaces are dropped, as with Latin text.
    pub(crate) fn wrap(&self, text: &str, size: f64, width: f64, latin: LatinWidth) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split(['\n', '\r']) {
            let breaks: HashSet<usize> = thai::break_offsets(para).into_iter().collect();
            // (text, width, a line may end after it)
            let mut cur: Vec<(String, f64, bool)> = Vec::new();
            let flush = |cur: &mut Vec<(String, f64, bool)>, lines: &mut Vec<String>, keep: usize| {
                let rest = cur.split_off(keep.min(cur.len()));
                let line: String = cur.iter().map(|a| a.0.as_str()).collect();
                lines.push(line.trim_end_matches(' ').to_string());
                *cur = rest;
            };
            for atom in self.atoms(para) {
                let w = atom.thai_em.map_or_else(|| latin(&atom.text, size), |em| em * size);
                let space = atom.text == " ";
                let allowed = space || breaks.contains(&atom.end);
                // Over the row: end the line at its last chance to break (or, in a word wider
                // than the row, right here between clusters).
                while !space && !cur.is_empty() && {
                    let trailing: f64 = cur.iter().rev().take_while(|a| a.0 == " ").map(|a| a.1).sum();
                    cur.iter().map(|a| a.1).sum::<f64>() - trailing + w > width
                } {
                    let keep = cur.iter().rposition(|a| a.2).map_or(cur.len(), |i| i + 1);
                    flush(&mut cur, &mut lines, keep);
                }
                cur.push((atom.text, w, allowed));
            }
            flush(&mut cur, &mut lines, usize::MAX);
        }
        lines
    }

    /// Draw one line with its left end at `x` and baseline at `y`: its Thai runs as filled
    /// outlines, every other run through `latin(out, x, run)`, which writes the run and returns
    /// its width. The whole line is one marked-content span carrying the typed text.
    pub(crate) fn draw_line(&self, out: &mut Vec<u8>, line: &str, x: f64, y: f64, size: f64, latin: &mut dyn FnMut(&mut Vec<u8>, f64, &str) -> f64) {
        if line.is_empty() {
            return;
        }
        let hex: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{hex}>>> BDC\n").bytes());
        let mut pen = x;
        for (is_thai, span) in spans(line) {
            if is_thai {
                pen += self.outlines_of(out, span, pen, y, size);
            } else {
                pen += latin(out, pen, span);
            }
        }
        out.extend_from_slice(b"EMC\n");
    }

    /// The paths of a Thai run, filled; returns the run's width.
    fn outlines_of(&self, out: &mut Vec<u8>, span: &str, x: f64, y: f64, size: f64) -> f64 {
        let Some(clusters) = self.shape(span) else { return 0.0 };
        let mut pen = x;
        let mut drew = false;
        for cluster in clusters.iter() {
            for (id, [dx, dy]) in &cluster.glyphs {
                let Some(glyph) = self.outline(*id) else { continue };
                for contour in glyph.contours.iter().filter(|c| c.len() >= 2) {
                    for (i, [px, py]) in contour.iter().enumerate() {
                        let (gx, gy) = (pen + (px + dx) * size, y + (py + dy) * size);
                        out.extend(format!("{} {} {}\n", fmt(gx), fmt(gy), if i == 0 { "m" } else { "l" }).bytes());
                    }
                    out.extend_from_slice(b"h\n");
                    drew = true;
                }
            }
            pen += cluster.advance * size;
        }
        if drew {
            out.extend_from_slice(b"f\n");
        }
        pen - x
    }
}
