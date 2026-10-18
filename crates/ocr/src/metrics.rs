//! Character error rate for judging recognition (the evaluation harness, not the product).

/// Edit distance between `expected` and `got`, counted in chars, divided by `expected`'s length.
pub fn cer(expected: &str, got: &str) -> f64 {
    let a: Vec<char> = expected.chars().collect();
    let b: Vec<char> = got.chars().collect();
    if a.is_empty() {
        return if b.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1; // bounded by the loop
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb); // bounded by the loop
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1); // bounded by the loop
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()] as f64 / a.len() as f64 // bounded by the loop
}

#[cfg(test)]
mod tests {
    use super::cer;

    #[test]
    fn cer_counts_thai_marks_as_characters() {
        assert_eq!(cer("", ""), 0.0);
        assert_eq!(cer("", "x"), 1.0);
        assert_eq!(cer("น้ำ", "น้ำ"), 0.0);
        assert!((cer("น้ำ", "นำ") - 1.0 / 3.0).abs() < 1e-9, "a dropped tone mark is one error in three");
        assert_eq!(cer("abc", "xyz"), 1.0);
    }
}
