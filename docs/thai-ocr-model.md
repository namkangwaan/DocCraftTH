# Thai recognition model

Checked 2026-10-17. Path B from the Thai OCR plan. The official weights load in rten after conversion. No character-error measurement yet.

## Candidate

Official weights from the PaddleOCR team.

- Model card: https://huggingface.co/PaddlePaddle/th_PP-OCRv5_mobile_rec
- Card revision: `9e25080455925d3943f5db885fddc79db6a07ca3`
- Licence on the card: Apache-2.0 (`license: apache-2.0` in the README front matter, and the Hugging Face API tag `license:apache-2.0`)
- Licence text: https://www.apache.org/licenses/LICENSE-2.0.txt
- Dictionary in the Apache-2.0 PaddleOCR repo: https://github.com/PaddlePaddle/PaddleOCR/blob/main/ppocr/utils/dict/ppocrv5_th_dict.txt

The published files are Paddle inference weights. The card has no `.onnx` file and no `.rten` file. Paddle publishes ONNX for `PP-OCRv5_mobile_rec` (Chinese, English, Japanese) at https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_rec_onnx. That package is not the Thai model.

| File | SHA-256 |
| --- | --- |
| `inference.pdiparams` | `45ec91f2322b58b8d30ba27d18fcfdbb8bf388b918dd978162d2af91e0c66d4b` |
| `inference.json` | `b92a9442af5ac6da3c6ba30ce1672d00a2d7a0fbd8068ed543ac6dcb9d5c7764` |
| `inference.yml` | `f6ba7fefc38ca1ff398ddafa75d67d16e0b3757c4e6c833adffee98a981766c9` |
| `ppocrv5_th_dict.txt` | `57f5406f94bb6688fb7077f7be65f08bbd71cecf48c01ea26c522cb5c4836b7a` |

Hashes were computed on the files downloaded to `K:\tmp\thai-ocr-model`. Those files are not in this repository.

## Geometry

`inference.yml` names the recognizer input `x` as NCHW. The height is 48. The listed widths are 160, 320, and 3200. The resize op uses image shape `[3, 48, 320]`. Decode is CTC. `character_dict` in that file has 524 entries, the same count as `ppocrv5_th_dict.txt`.

## Conversion

`inference.json` is Paddle PIR (`magic` is `pir`). `paddle2onnx` 2.1.0 does not load against Paddle 3.3.1 on Windows (`paddle2onnx_cpp2py_export` reports a missing procedure). The same converter loads against Paddle 3.1.1 and writes ONNX.

The export is opset 9. Its eight `Slice` nodes store `axes`, `starts`, and `ends` as attributes. `rten-convert` 0.22 drops those attributes. ONNX `version_converter` lifts the file to opset 11, where those slices take the three values as inputs. `rten-convert` 0.26.0 is not published. Version 0.22.0 writes a file that rten 0.26 reads. Shape inference inside `rten-convert` was skipped because its temporary file cannot be reopened on Windows.

| Derived file | SHA-256 |
| --- | --- |
| `th_PP-OCRv5_mobile_rec.onnx` (opset 9, constant-folded, 577 nodes) | `10187bf5af9ce592c2c6d2c898efed5cab222715e823e5b6b964b72154c0e56c` |
| `th_PP-OCRv5_mobile_rec_opset11.onnx` | `9d323430ec82b04f09c347a1de4159d19a0e3b7cf170881335116341bd5cc298` |
| `th_PP-OCRv5_mobile_rec.rten` | `8b70e6d18967e93fccb32e045e0f80ee3859efe7ccdd67586a9d136a284e834a` |

The derived files stay in `K:\tmp\thai-ocr-model`. They are not repository assets and have no `[[fetched]]` entry.

## Load check

A scratch program outside the repository, `K:\tmp\thai-ocr-probe`, loads `th_PP-OCRv5_mobile_rec.rten` with `rten` 0.26.0, feature `rten_format` only. `cargo check` finished, then one `cargo run`.

- Input `x` shape is `["DynamicDimension.0", 3, 48, "DynamicDimension.1"]`.
- Output `fetch_name_0` shape is `["DynamicDimension.0", "unknown_2", 526]`.
- A zero tensor `[1, 3, 48, 160]` runs and returns `[1, 20, 526]`.

The output depth is 526. The dictionary has 524 entries, so the depth is the dictionary length plus two, not plus one. The class index map is still open. Greedy CTC decoding has not been written.

## Ruling

The recognizer loads. There is no character-error rate, because detection boxes and CTC decoding are not wired, and the synthetic pages still have no Thai face (`pdfcraft_fonts::document_thai_font()` is empty without `CRAFT_FONTS_DIR`). No `[[fetched]]` entry until the 0.05 mean CER gate is measured.

Path A, training an ocrs-format model, is not started.
