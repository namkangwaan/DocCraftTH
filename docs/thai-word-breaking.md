# Thai word breaking

> **Last reviewed:** 2026-10-17 · **Last updated:** 2026-10-17 · **Change:** minor (first oracle run: the ICU4X dictionary segmenter breaks mid-word after unknown words; LSTM does not) · **Status:** Part 1 (`break_offsets`, Edit text) on `feat/thai-line-breaking`; Add text fix and Part 2 (UI labels, comments) in progress on `wip/thai-line-breaking-part2`

Thai is written without spaces between words. A line may end between two words, but never inside a word and never before a tone mark or a vowel sign written above or below its letter. DocCraftTH finds the word boundaries with one function and uses it in four places.

## The engine

`pdfcraft_fonts::thai::break_offsets(text)` (`crates/fonts/src/thai.rs`) returns the byte offsets where a line may end next to a Thai character. It runs the ICU4X dictionary line segmenter, `icu_segmenter::LineSegmenter::new_dictionary` (icu_segmenter 2.3.0, `compiled_data`), which is UAX #14 line breaking with ICU's Thai word list. It then filters the segmenter's answer: offsets stay strictly inside the text and on char boundaries, never fall before a combining mark (U+0E31, U+0E34–U+0E3A, U+0E47–U+0E4E) and never directly after whitespace. A wrong answer from the segmenter can therefore drop a break, but cannot split a cluster on those marks.

Why ICU4X's dictionary segmenter:

- Rust, offline, no new runtime. `icu_properties` and `icu_provider` were already in the tree through egui, so only the segmenter and its data are new.
- It builds for wasm. The linked size cost, measured with a probe crate (`K:\tmp\wasm-probe`, after the Part 1 commit `5226aba1` recorded it as not measured), is +1.76 MiB of the 2 MiB budget the line-breaking plan set. `LineSegmenter::new_lstm` was the fallback if it went over; it did not.
- The data is under the Unicode licence, already accepted for ICU4X.

Rejected:

- LLMs such as the openthai.ai models: gigabytes, a separate runtime or a network call, and BPE tokens that are not words.
- PyThaiNLP inside the product: Python. It is used only as an external oracle (below).
- Porting newmm or another segmenter: clean-room risk (below) for no measured gain.

## The four wrap paths

| What wraps | Where | How it uses the breaks | State |
|---|---|---|---|
| Add text (new text written into a PDF) | `crates/edit/src/added.rs`, `thai_lines` | Shapes the paragraph, groups shaped clusters into words at `break_offsets` and at spaces, and fills rows word by word. A word wider than the row breaks at shaped cluster boundaries, which never detach a mark. | `74ba36c6` on `wip/thai-line-breaking-part2` |
| Edit text (rewrapping existing page text) | `crates/edit/src/text.rs`, `wrap`, through `crates/edit/src/linebreak.rs`, `pieces` | `pieces` cuts a space-separated word at `break_offsets` as well as at the CJK rules (UAX #14 ID, kinsoku). | `051994f7` on `feat/thai-line-breaking` |
| UI labels (the `th` catalog) | `crates/ui-egui/src/i18n/th_breaks.rs` and `vendor/epaint/src/text/text_layout.rs` | `soft_breaks` inserts U+200B ZERO WIDTH SPACE at each break. egui wraps only at whitespace, so the vendored epaint 0.36.2 treats U+200B as a break candidate and never starts a row with a combining mark (two `PdfCraft patch:` edits). Text that leaves the screen (signature appearance labels written into a PDF, control-channel matches) goes through `without_soft_breaks`. | Part 2, in progress |
| Comments (annotation contents) | `crates/ui-egui/src/bidi.rs`, `words` and `fitting_prefix` | `words` splits Thai runs at `break_offsets`; `fitting_prefix` breaks an overlong word but never before a mark. | Part 2, in progress |

All four call the same function, so a fix to the engine or a supplementary word list reaches all of them.

## Measuring it: the PyThaiNLP oracle

[PyThaiNLP](https://pythainlp.org/)'s `newmm` tokenizer (maximal matching over its own word list) is the reference. It runs outside the product, as an external process, the way MuPDF and veraPDF do. Neither segmenter is ground truth: newmm keeps compounds such as กินข้าว together, and ICU may split them. Breaking at a compound seam is fine for wrapping. What matters is a break inside a word.

The oracle compares, for every unique Thai translation in `crates/ui-egui/src/i18n/th.tsv`, the breaks both engines put between two Thai characters, and sorts every disagreement into a class:

| Class | Meaning |
|---|---|
| a | ICU break that splits a Thai character cluster (TCC, from `pythainlp.tokenize.tcc`). A bug in the product. |
| b | ICU-only break inside a newmm dictionary word where a side is not itself a word. Likely mid-word. |
| c | ICU-only break at a compound seam, both sides words. Fine for wrapping. |
| e | ICU-only break inside a run newmm could not segment. No verdict. |
| d | newmm-only break. A missed wrap opportunity, harmless. |

### First run (2026-10-17)

pythainlp 5.4.0, `th.tsv` at `1d0a35bc`, `break_offsets` at `d802ee7c`, 2,464 unique translations. Report: `K:\tmp\thai-break-oracle.md`, every row in `thai-break-oracle.json`.

| | Dictionary (shipped) | LSTM (`new_lstm`, tested only) |
|---|---|---|
| Strings where both engines agree | 1,145 (46.5%) | 890 (36.1%) |
| Precision / recall vs newmm | 76.1% / 95.5% | 75.2% / 92.2% |
| a: splits a cluster | 191 breaks in 127 strings | 0 |
| b: inside a dictionary word | 258 | 143 |
| Strings with any a or b break | 200 (8.1%) | 132 (5.4%) |

**ICU's dictionary segmenter is wrong on real UI words.** After a word it does not know (transliterated loanwords such as บุ๊กมาร์ก, ออบเจ็กต์, ไดเจสต์, ไฮไลต์, and ลงนาม), icu_segmenter 2.3.0 loses step and cuts the rest of the run in the wrong places: `บุ๊กมาร์กบางรายการไม่ได้แสดงที่นี่` → `บุ๊ก|มา|ร์|กบ|า|งร|า|ยก|า|รไ|ม่ไ|ด้แ|สด|งที่|นี่`, while the same sentence without the loanword segments correctly. The raw `segment_str` output has the same fault, so it is ICU4X's, not the filter's. Consequences:

- A guard against breaks before ะ า ำ ๅ ๆ ฯ and after เ แ โ ใ ไ catches 104 of the 191 cluster splits and none of the mid-word breaks. It is a safety net, not a fix.
- A supplementary word list hides the known triggers but not the desync. The next loanword sets it off again.
- `LineSegmenter::new_lstm` (icu_segmenter feature `lstm`) has no cluster splits on this catalog. Its wasm cost is not measured yet; measure it against the 2 MiB budget before switching.

### Rerunning the oracle

Nothing here is part of the build. Python stays outside the repository.

1. A venv outside the repository: `uv venv K:\tmp\thai-oracle\venv`, then `uv pip install --python K:\tmp\thai-oracle\venv\Scripts\python.exe pythainlp`.
2. The Rust side, an untracked example in `crates/fonts/examples/thai_breaks.rs` (never committed):

   ```rust
   //! Oracle helper, never committed: reads one string per stdin line and prints
   //! `pdfcraft_fonts::thai::break_offsets` for it as a JSON array of byte offsets.
   //! `--raw` prints the ICU4X dictionary segmenter's unfiltered offsets, `--lstm` the LSTM one's.
   use icu_segmenter::LineSegmenter;
   use icu_segmenter::options::LineBreakOptions;
   use std::io::BufRead;

   fn main() {
       let mode = std::env::args().nth(1).unwrap_or_default();
       let dict = LineSegmenter::new_dictionary(LineBreakOptions::default());
       let lstm = LineSegmenter::new_lstm(LineBreakOptions::default());
       for line in std::io::stdin().lock().lines().map_while(Result::ok) {
           let offsets: Vec<usize> = match mode.as_str() {
               "--raw" => dict.segment_str(&line).collect(),
               "--lstm" => lstm.segment_str(&line).collect(),
               _ => pdfcraft_fonts::thai::break_offsets(&line),
           };
           let offsets: Vec<String> = offsets.iter().map(usize::to_string).collect();
           println!("[{}]", offsets.join(","));
       }
   }
   ```

   Build it with `cargo build -p pdfcraft-fonts -j 2 --example thai_breaks` and its own `CARGO_TARGET_DIR`. `new_lstm` needs the icu_segmenter feature `lstm`: add it to `crates/fonts/Cargo.toml` for the run only and revert it afterwards. The raw and LSTM modes include the offsets 0 and the text length; drop them before `compare`.
3. The script `oracle.py` (kept in `K:\tmp\thai-oracle\` and in the session notes, not in the repository) does the rest. Run Python with `-I -X utf8`.
   - `oracle.py extract th.tsv strings.txt` writes the unique third-column translations, one per line, after a canary line ผมกินข้าว.
   - Feed `strings.txt` to the example with shell redirection, not a PowerShell pipe, which re-encodes stdin: `thai_breaks.exe < strings.txt > icu.jsonl`.
   - `oracle.py compare strings.txt icu.jsonl thai-break-oracle.md` checks that the canary came back as `[6,15]` (ผม|กิน|ข้าว) and that few strings came back with no break at all. Both checks catch mangled input, which would otherwise read as "ICU misses everything". It then classifies each disagreement and writes the report and its JSON.

The script can be rewritten from this description: newmm with `keep_whitespace=True` for the reference breaks, `tcc_pos` for class a, `pythainlp.corpus.thai_words()` (the CC0 `words_th.txt`) for classes b and c, and only boundaries between two Thai characters that are not before a mark.

## PyThaiNLP data and licences

From `pythainlp/corpus/corpus_license.md` in pythainlp 5.4.0.

| Data | Licence | Use here |
|---|---|---|
| `words_th.txt`, `tnc_freq.txt`, `ttc_freq.txt`, `syllables_th.txt`, `etcc.txt`, `stopwords_th.txt` | CC0-1.0 | Usable, for example as a supplementary word list, each with an `ATTRIBUTION.toml` entry (source, licence, sha256). |
| `family_names_th.txt`, `person_names_*_th.txt`, `wikipedia_titles_th.txt`, `volubilis_words_th.txt` | CC BY-SA 4.0 | Not used. ShareAlike would bind the catalog and the binary. |
| Models (`pos_*`, `sentenceseg_crfcut.model`, …) | CC BY 4.0 | Not used. |
| `icubrk_th.txt` | Unicode licence (ICU `thaidict.txt`) | Not needed. ICU4X already ships this data. |
| `wordnet_th.db` | NICT Thai WordNet licence | Not used. |
| `orst_words_th.txt`, `phupha_word_freqs.txt` | Not stated in `corpus_license.md` | Not used. |
| LST20 corpus (not in the pythainlp package) | Custom terms on its [dataset card](https://huggingface.co/datasets/lst-nectec/lst20): free for non-commercial use, commercial use paid, no redistribution or modification without permission. Not CC BY-NC-SA, as an earlier note assumed. | Not usable. |

## Clean-room

- Use PyThaiNLP only through its outputs (token boundaries, TCC boundaries) and its CC0 data files.
- Never read newmm's or any other segmenter's source to write product code, and never port it. The oracle script calls the public API only.
- A word list taken into the product comes only from CC0 data, is recorded in `ATTRIBUTION.toml`, and never from CC BY-SA, non-commercial or unlicensed lists.

## When to add a supplementary word list

Only if the oracle shows ICU splitting real UI words (class a, or class b rows a Thai reader confirms), and only from CC0 data. Fix class a in `break_offsets` itself, since it is a cluster bug and not a vocabulary gap. Prefer rewording a catalog string over a word list when only a handful of rows are affected. A list would suppress breaks inside the listed words, so it can only remove wrap opportunities, never add a mid-word one.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-17 | minor | First oracle run: the ICU4X dictionary segmenter desyncs after unknown words (191 cluster splits in 127 of 2,464 strings); LSTM has none on the same catalog. Rerun steps cover `--raw` and `--lstm`. |
| 2026-10-17 | major | First version: ICU4X dictionary engine and why (wasm +1.76 MiB of 2 MiB), the four wrap paths, the PyThaiNLP oracle and how to rerun it, PyThaiNLP corpus licences, clean-room rule. |
