# Thai recognition model

Checked 2026-10-17. Path B from the Thai OCR plan. No engine rebuild was run for this check.

## Candidate

Official weights from the PaddleOCR team.

- Model card: https://huggingface.co/PaddlePaddle/th_PP-OCRv5_mobile_rec
- Card revision: `9e25080455925d3943f5db885fddc79db6a07ca3`
- Licence on the card: Apache-2.0 (`license: apache-2.0` in the README front matter, and the Hugging Face API tag `license:apache-2.0`)
- Licence text: https://www.apache.org/licenses/LICENSE-2.0.txt
- Dictionary in the Apache-2.0 PaddleOCR repo: https://github.com/PaddlePaddle/PaddleOCR/blob/main/ppocr/utils/dict/ppocrv5_th_dict.txt

The published files are Paddle inference weights. There is no `.onnx` file and no `.rten` file.

| File | SHA-256 |
| --- | --- |
| `inference.pdiparams` | `45ec91f2322b58b8d30ba27d18fcfdbb8bf388b918dd978162d2af91e0c66d4b` |
| `inference.json` | `b92a9442af5ac6da3c6ba30ce1672d00a2d7a0fbd8068ed543ac6dcb9d5c7764` |
| `inference.yml` | `f6ba7fefc38ca1ff398ddafa75d67d16e0b3757c4e6c833adffee98a981766c9` |
| `ppocrv5_th_dict.txt` | `57f5406f94bb6688fb7077f7be65f08bbd71cecf48c01ea26c522cb5c4836b7a` |

Hashes were computed on the files downloaded to `K:\tmp\thai-ocr-model`. Those files are not in this repository.

## Geometry

`inference.yml` names the recognizer input `x` as NCHW. The height is 48. The listed widths are 160, 320, and 3200. The resize op uses image shape `[3, 48, 320]`. Decode is CTC. The character list in that file includes Thai consonants, vowels, tone marks, and Thai digits.

## Why rten cannot load it

`crates/ocr/Cargo.toml` depends on `rten` 0.26.0 with default features off and only `rten_format`. That build reads `.rten` files. The rten 0.26.0 docs say `.onnx` loading needs the `onnx_format` feature, which this crate does not enable.

Paddle publishes an ONNX package for `PP-OCRv5_mobile_rec` (Chinese, English, Japanese) at https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_rec_onnx. That package is not the Thai model.

## Ruling

Path B stops here. The official Thai weights are not a file this build can load, so there is no character-error measurement and no `[[fetched]]` entry.

Path A, training an ocrs-format model, is not started. It needs a separate plan and a training machine.
