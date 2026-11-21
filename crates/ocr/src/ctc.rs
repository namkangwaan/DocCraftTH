//! Greedy CTC decoding for line recognizers whose output is `[T, classes]` probabilities with
//! class 0 the blank (the Thai recognizer; ocrs decodes its own).

/// The text in `probs` (`T` rows of `classes` scores, row-major). Each row's best class wins
/// (NaN never wins); repeats of the same class merge; blanks separate. Class `k` is `dict[k - 1]`;
/// a class past the dictionary, and a trailing partial row, are skipped.
pub fn decode(probs: &[f32], classes: usize, dict: &[char]) -> String {
    decode_scored(probs, classes, dict).0
}

/// [`decode`] and a confidence in `0..=1`: the mean winning probability of the characters that
/// were emitted (0 when none were). A CTC model that is sure of every character it writes scores
/// near 1; a page read in the wrong orientation, or noise, scores low.
pub fn decode_scored(probs: &[f32], classes: usize, dict: &[char]) -> (String, f32) {
    let mut out = String::new();
    if classes == 0 {
        return (out, 0.0);
    }
    let (mut last, mut sum, mut n) = (0usize, 0.0f64, 0usize);
    for row in probs.chunks_exact(classes) {
        let (best, p) =
            row.iter().enumerate().filter(|(_, p)| p.is_finite()).max_by(|a, b| a.1.total_cmp(b.1)).map_or((0, 0.0), |(k, p)| (k, f64::from(*p)));
        if best != 0
            && best != last
            && let Some(c) = best.checked_sub(1).and_then(|i| dict.get(i))
        {
            out.push(*c);
            sum += p.clamp(0.0, 1.0);
            n += 1;
        }
        last = best;
    }
    (out, if n == 0 { 0.0 } else { (sum / n as f64) as f32 })
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
    fn scored_decoding_reports_the_mean_winning_probability() {
        let dict = ['ก', 'า'];
        let flat = [0.05, 0.9, 0.05, 0.0, 0.8, 0.1, 0.1, 0.0, 0.1, 0.0, 0.6, 0.3f32];
        let (text, p) = super::decode_scored(&flat, 4, &dict);
        assert_eq!(text, "กา");
        assert!((p - 0.75).abs() < 1e-6, "{p}");
        assert_eq!(super::decode_scored(&[1.0, 0.0, 0.0, 0.0], 4, &dict), (String::new(), 0.0));
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
