# Thai recognition model

Checked 2026-10-17. Path B from the Thai OCR plan (`Thai recognizer = B`). The official weights load in rten after conversion and pass the 0.05 mean CER gate (mean CER `0.0136`).

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
| `th_PP-OCRv5_mobile_rec.rten` (`text-recognition-th.rten`) | `8b70e6d18967e93fccb32e045e0f80ee3859efe7ccdd67586a9d136a284e834a` |

The model card publishes only the Paddle weights, so the converted `.rten` is hosted as the `text-recognition-th.rten` asset of this fork's `models-th-v1` release. `cargo xtask models` fetches it from there.

## Load check and class map

A scratch program outside the repository, `K:\tmp\thai-ocr-probe`, loads `th_PP-OCRv5_mobile_rec.rten` with `rten` 0.26.0, feature `rten_format` only.

- Input `x` shape is `["DynamicDimension.0", 3, 48, "DynamicDimension.1"]` (BGR channels normalized with `(pixel / 255.0 - 0.5) / 0.5`, height 48 px, minimum padded width 160 px).
- Output `fetch_name_0` shape is `["DynamicDimension.0", "unknown_2", 526]`.
- A zero tensor `[1, 3, 48, 160]` runs and returns `[1, 20, 526]`.
- Class `0` is the CTC blank, classes `1..=524` map to the 524 lines of `ppocrv5_th_dict.txt` (`dict-th.txt`), and class `525` is the space character (`' '`, from PaddleOCR's `use_space_char: true`).

## Detection and CER gate (`crates/engine/tests/thai_ocr.rs`)

Detected line bounding boxes from `ocrs` are padded by 25 % of box height above and 15 % below (plus `max(0.15 * height, 2.0)` horizontally) so Thai combining marks above and below the base line are not clipped.

Measured at 300 dpi with `CRAFT_FONTS_DIR=K:\src\craft-fonts` (Noto Sans Thai):

| Sentence | CER |
| --- | --- |
| `รายงานประจำปี ๒๕๖๙` | `0.0000` |
| `ใบแจ้งหนี้ No. 2569/001` | `0.0000` |
| `น้ำใจ ปฏิบัติ ฤๅษี ฎีกา` | `0.0435` |
| `กรุณาลงนามและส่งคืนภายในวันที่ 30 กันยายน` | `0.0000` |
| `ข้อมูลส่วนบุคคลได้รับการคุ้มครองตามกฎหมาย` | `0.0244` |
| **Mean** | **`0.0136`** (gate ≤ `0.05`) |

## Installing the model

The app looks for `text-recognition-th.rten`, `dict-th.txt` and the shared `text-detection.rten` in `$PDFCRAFT_MODELS`, in `models/` beside the executable (release packages) and in `assets/models/` of the source tree. `ocr_status` lists the directories it searched and whether `th` is available. Without the Thai files the Recognize Text dialog, on Thai, says that the Thai model is not installed and English keeps working.

`cargo xtask models` fetches `dict-th.txt` and the English models. The `[[fetched]]` entry of `text-recognition-th.rten` still points at the Paddle weights the conversion started from, so a fresh fetch stops on the SHA-256 mismatch until the converted file has a download address (read from `xtask/src/assets.rs`, not run). Until then copy the converted file into one of the directories above.

## Ruling

`Thai recognizer = B` (`text-recognition-th.rten` + `dict-th.txt`, Apache-2.0). Mean CER is `0.0136`, well within the `0.05` gate. Path A (training an ocrs-format model) is not needed.
