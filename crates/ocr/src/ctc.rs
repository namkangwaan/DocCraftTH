//! Greedy CTC decoding for line recognizers whose output is `[T, classes]` probabilities with
//! class 0 the blank (the Thai recognizer; ocrs decodes its own).

/// The text in `probs` (`T` rows of `classes` scores, row-major). Each row's best class wins
/// (NaN never wins); repeats of the same class merge; blanks separate. Class `k` is `dict[k - 1]`;
/// a class past the dictionary, and a trailing partial row, are skipped.
pub fn decode(probs: &[f32], classes: usize, dict: &[char]) -> String {
    let mut out = String::new();
    if classes == 0 {
        return out;
    }
    let mut last = 0usize;
    for row in probs.chunks_exact(classes) {
        let best = row.iter().enumerate().filter(|(_, p)| p.is_finite()).max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(k, _)| k);
        if best != 0
            && best != last
            && let Some(c) = best.checked_sub(1).and_then(|i| dict.get(i))
        {
            out.push(*c);
        }
        last = best;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn greedy_ctc_collapses_repeats_and_blanks() {
        let dict = ['ก', 'า', 'ร'];
        // 4 classes (blank + 3), 6 time steps: ก ก blank า ร ร → "การ"
        let rows: [[f32; 4]; 6] =
            [[0.0, 0.9, 0.0, 0.1], [0.0, 0.9, 0.0, 0.1], [0.9, 0.0, 0.0, 0.1], [0.0, 0.0, 0.9, 0.1], [0.0, 0.0, 0.0, 0.9], [0.0, 0.0, 0.0, 0.9]];
        let flat: Vec<f32> = rows.iter().flatten().copied().collect();
        assert_eq!(decode(&flat, 4, &dict), "การ");
    }

    #[test]
    fn malformed_output_never_panics() {
        assert_eq!(decode(&[], 4, &['ก']), "");
        assert_eq!(decode(&[0.1, 0.9, 0.0], 4, &['ก']), "", "partial row ignored");
        assert_eq!(decode(&[0.0, 0.0, 0.0, 0.9], 4, &['ก']), "", "class past the dictionary skipped");
        assert_eq!(decode(&[f32::NAN, 0.5, 0.0, 0.0], 4, &['ก']), "ก", "NaN never wins");
        assert_eq!(decode(&[1.0], 0, &['ก']), "", "zero classes");
    }
}
