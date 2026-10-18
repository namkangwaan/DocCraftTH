//! Where a line may end inside Thai text, which is written without spaces between words. The
//! ICU4X LSTM line segmenter (UAX #14 with a Thai word model) finds the word boundaries. Its
//! dictionary segmenter loses step after a word it does not know (บุ๊กมาร์ก, ออบเจ็กต์) and then
//! splits clusters for the rest of the run; see docs/thai-word-breaking.md.

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;

/// The Thai block (U+0E00..U+0E7F).
pub fn is_thai(c: char) -> bool {
    ('\u{0E00}'..='\u{0E7F}').contains(&c)
}

/// Thai vowel signs and tone marks written above or below their letter (general category Mn).
pub fn is_mark(c: char) -> bool {
    matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

/// Whether a line may end between `before` and `after` without splitting a Thai syllable: no
/// line starts with a mark, a following vowel (ะ า ำ ๅ), ๆ or ฯ, and none ends with a leading
/// vowel (เ แ โ ใ ไ). Any other pair may break.
pub fn may_break(before: char, after: char) -> bool {
    !matches!(before, '\u{0E40}'..='\u{0E44}')
        && !is_mark(after)
        && !matches!(after, '\u{0E2F}' | '\u{0E30}' | '\u{0E32}' | '\u{0E33}' | '\u{0E45}' | '\u{0E46}')
}

/// Byte offsets strictly inside `text` where a line may end next to a Thai character. Always on
/// a char boundary and where [`may_break`] allows; the segmenter's own result is filtered, so a
/// bad answer from it can drop a break but never split a syllable.
pub fn break_offsets(text: &str) -> Vec<usize> {
    if !text.chars().any(is_thai) {
        return Vec::new();
    }
    let segmenter = LineSegmenter::new_lstm(LineBreakOptions::default());
    segmenter
        .segment_str(text)
        .filter(|&at| at > 0 && at < text.len() && text.is_char_boundary(at))
        .filter(|&at| {
            let before = text.get(..at).and_then(|s| s.chars().next_back());
            let after = text.get(at..).and_then(|s| s.chars().next());
            match (before, after) {
                (Some(b), Some(a)) => (is_thai(b) || is_thai(a)) && may_break(b, a) && !b.is_whitespace(),
                _ => false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut start = 0;
        for at in break_offsets(text) {
            out.extend(text.get(start..at));
            start = at;
        }
        out.extend(text.get(start..));
        out
    }

    #[test]
    fn thai_breaks_between_words() {
        assert_eq!(pieces("ผมกินข้าว"), ["ผม", "กิน", "ข้าว"]);
        assert!(pieces("รายงานประจำปี").len() >= 2, "{:?}", pieces("รายงานประจำปี"));
        assert!(break_offsets("hello world").is_empty(), "Latin is not this helper's job");
        assert!(break_offsets("").is_empty());
    }

    #[test]
    fn thai_breaks_never_split_a_cluster() {
        for text in ["น้ำใจ", "ที่นี่", "ปั้นดิน", "ฤๅษีมาแล้ว", "กำลังพยายามเปิดไฟล์ในเครื่อง"]
        {
            for at in break_offsets(text) {
                let next = text.get(at..).and_then(|rest| rest.chars().next());
                assert!(next.is_some_and(|c| !is_mark(c)), "{text:?} breaks before a mark at {at}");
            }
            assert_eq!(pieces(text).concat(), text);
        }
    }

    #[test]
    fn thai_breaks_keep_syllables_whole() {
        assert!(!may_break('ไ', 'ป'), "a leading vowel never ends a line");
        for after in ['ะ', 'า', 'ำ', 'ๅ', 'ๆ', 'ฯ', '\u{0E49}'] {
            assert!(!may_break('ก', after), "{after:?} never starts a line");
        }
        assert!(may_break('ป', 'ไ'));
        for text in ["ไปๆมา", "กรุงเทพฯและเชียงใหม่", "เกเรเกเร", "ตระการตาน้ำใจ"]
        {
            for at in break_offsets(text) {
                let before = text.get(..at).and_then(|s| s.chars().next_back());
                let after = text.get(at..).and_then(|s| s.chars().next());
                assert!(before.zip(after).is_some_and(|(b, a)| may_break(b, a)), "{text:?} splits a syllable at {at}: {:?}", pieces(text));
            }
        }
    }

    #[test]
    fn thai_breaks_handle_hostile_input() {
        let long = "ก".repeat(100_000);
        let _ = break_offsets(&long);
        assert!(break_offsets("่่่").is_empty());
        let _ = break_offsets("\u{0E01}\u{0301}\u{0E48}x\u{200B}ก");
    }
}
