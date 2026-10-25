<h1 align="center">DocCraftTH — Thai OCR & Searchable PDF Enhancement</h1>

<p align="center">
  <b>การปรับปรุงประสิทธิภาพภาษาไทยและระบบรู้จำอักขระ (Thai OCR) สำหรับเอกสารราชการและกฎหมายไทย</b><br>
  ต่อยอดจากโครงการต้นฉบับโอเพนซอร์ส <a href="https://github.com/storytold/pdfcraft">PdfCraft by Storytold / ArtCraft</a>
</p>

---

> [!IMPORTANT]
> ### 📢 ข้อความระบุแหล่งที่มาและประกาศชี้แจงสถานะโครงการ (Attribution & Fork Disclaimer)
> 
> **โครงการนี้ไม่ใช่ผลงานต้นฉบับดั้งเดิม และไม่ใช่คลังทางการของโครงการ PdfCraft / OpenCraft / ArtCraft**
> 
> คลังโค้ดนี้ได้รับการพัฒนาต่อยอด (Forked & Downstream Enhancement) ภายใต้สัญญาอนุญาตแบบ Dual License ([MIT](https://opensource.org/licenses/MIT) / [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0)) จากโครงการต้นฉบับ:
> - **โครงการต้นฉบับหลัก (Upstream Original Repository):** [`storytold/pdfcraft`](https://github.com/storytold/pdfcraft)
> - **ผู้ถือลิขสิทธิ์และทีมพัฒนาต้นฉบับ:** Copyright (c) 2026 ArtCraft Team and the PdfCraft contributors (เว็บไซต์: [getartcraft.com](https://getartcraft.com/))
> 
> **วัตถุประสงค์ของการพัฒนาเฉพาะกิจ (Purpose of this Thai Enhancement):**
> โครงการนี้จัดทำขึ้นเพื่อเป็น **งานวิจัยทางวิศวกรรมและการพัฒนาเฉพาะด้านภาษาไทย (Thai Language & Thai OCR Engineering)** โดยไม่ถือเป็นการคัดลอกระบบเพื่อแข่งขันหรือแอบอ้างสิทธิ์ แต่เป็นการพัฒนาส่วนขยายเพื่อแก้ปัญหาเฉพาะทางของภาษาไทยสำหรับเอกสารราชการ เอกสารกฎหมาย และการแปลงไฟล์สแกนเป็น Searchable PDF (`Ctrl+F` ได้อย่างแม่นยำ) ภายในระบบงานนี้เท่านั้น
> 
> *(โครงการนี้จัดทำขึ้นเพื่อการใช้งานเฉพาะกิจภายใน และเตรียมปรับสถานะเป็น Private Repository)*

---

## 🔬 สรุปผลการวิจัยและพัฒนาเฉพาะภาษาไทย (Thai Engineering Enhancements)

โครงการนี้ได้ทำการปรับปรุงและพัฒนาเพิ่มเติมใน 10 มิติหลัก:

### 1. การปรับจูน Bias โมเดลรู้จำอักขระไทย (CTC Classifier Tuning)
- ปรับแต่งค่าน้ำหนักเวกเตอร์ `linear_8.b_0` ในโมเดล RTen (`text-recognition-th.rten`)
- เพิ่ม Bias เลขไทย `๐-๙` (+1.2) เพื่อลดอัตราความสับสนระหว่างเลขอารบิกและเลขไทยในเอกสารราชการ
- ระงับสัญญาณรบกวนของสัญลักษณ์คณิตศาสตร์และอักขระขยะ (indices 88..441)

### 2. การสร้างคลังคำไทยมหาศาล (Mega Thai Lexicon Vault — 811,035 คำ)
- รวบรวมและ Normalization คำศัพท์จากแหล่งข้อมูลมาตรฐานระดับชาติ:
  - **พจนานุกรมราชบัณฑิตยสถาน 2542 & ศัพท์บัญญัติ (ORST):** 206,599 คำ
  - **คลังศัพท์กฎหมาย NitiBench (VISTEC):** 3,890 คำ (กฎหมายแรงงาน ภาษี คำพิพากษาศาลฎีกา)
  - **ฐานข้อมูลภูมิศาสตร์ไทย (Thailand-Geography-JSON):** 20,597 รายการ (ตำบล อำเภอ จังหวัด รหัสไปรษณีย์)
  - **คลังคำย่อราชการไทย:** 599 คำ (พ.ร.บ., ก.พ., ครม., นิติฯ ฯลฯ)
  - **คลังรายชื่อบุคคลไทย:** 266,583 รายการ (ป้องกันการตัดคำผิดพลาดบริเวณส่วนลงนาม)
  - **คลังสารานุกรมและ Wikipedia ภาษาไทย:** 407,379 คำ
  - **Active High-Precision Lexicon:** คัดเลือก 239,334 คำ สำหรับโหลดเข้าสู่หน่วยความจำ (ความเร็วโหลด 2.07 วินาที)

### 3. ระบบกู้คืนพิกัดและแก้วรรณยุกต์สระไทย (2D Geometric Reading Order & Grapheme Lock)
- พัฒนาอัลกอริทึมซ่อมแซมลำดับสระ-วรรณยุกต์ที่สลับตำแหน่ง (Mark Swaps)
- ซ่อมแซมรูปสระอำ (`\u0e4d\u0e32` $\rightarrow$ `\u0e33`) และจัดลำดับวรรณยุกต์ให้เป็นมาตรฐาน Unicode NFC
- ผสานตำแหน่งกล่องข้อความจากโมเดล OCR ให้สอดคล้องกับเลเยอร์ข้อความล่องหน (Invisible Text Layer) บน PDF

### 4. ผลการทดสอบความแม่นยำ (Empirical Benchmark Results)
ทดสอบวัดผลบน Shared Evaluation Corpus 11 หน้าเอกสารราชการและเอกสารสแกนจริง เปรียบเทียบกับ Tesseract 5.4.0 (tha):

| เอกสารทดสอบ | PdfCraft Baseline | **Thai Enhanced (ระบบนี้)** | Tesseract 5.4.0 (Oracle) | สถานะเทียบเกณฑ์ (Gate $\le 0.10$) |
| :--- | :---: | :---: | :---: | :---: |
| **ราชกิจจานุเบกษา (Ratchakitcha P.1)** | 0.1017 | **0.0890** | 0.0137 | **PASS (ผ่านเกณฑ์)** |
| **หนังสือกฎหมาย/คำสั่ง (Attachment P.3)** | 0.2748 | **0.0807** | 0.0794 | **PASS (ผ่านเกณฑ์)** |
| **หนังสือระเบียบการเงิน (Payroll P.1)** | 0.0655 | **0.0655** | 0.0600 | **PASS (ผ่านเกณฑ์)** |
| **ค่าเฉลี่ยเอกสาร 11 หน้า (Mean CER)** | 0.2000 | **0.1529** | 0.1925 | **ชนะ Tesseract Baseline** |
| **ความแม่นยำระดับบรรทัด (Line CER)** | 0.1420 | **0.0783** | 0.1066 | **ความถูกต้อง > 92.1%** |

### 5. การพัฒนาชุดเครื่องมือติดตั้งสำหรับ Windows
- จัดทำสคริปต์ Release Build (`build_release.ps1`) รองรับคำสั่ง CPU SIMD AVX2
- จัดทำสคริปต์ Windows System Installer (`install_system.ps1`) สำหรับติดตั้งลงสู่ `C:\Program Files\PdfCraft` พร้อมกำหนดค่า System Environment (`PATH`, `PDFCRAFT_MODELS`) โดยอิสระ

### 6. การแสดงผล การตัดคำ และค้นหาข้อความไทย
- **ฟอนต์ Sarabun (OFL-1.1)** ควบคู่ Noto Sans Thai: ตัวอักษรที่ Noto Sans Thai ไม่มีจะ fallback ไป Sarabun อัตโนมัติ (`crates/fonts`)
- **การตัดคำและตัดบรรทัดภาษาไทย (Thai Word & Line Breaking):** รองรับการตัดบรรทัดภาษาไทยด้วยโมเดล LSTM ควบคู่พจนานุกรม (`thai_wrap.rs` และแพตช์ `vendor/epaint`)
- **ช่องกรอกแบบฟอร์ม (AcroForm)** แสดงตัวอักษรไทยได้ถูกต้อง ไม่เป็น `????` และมี `/ActualText` สำหรับค้นหา พร้อมรองรับเส้นประช่องกรอกข้อมูล (Form Leader Fields)
- **FR-18 PUA Mapping:** แปลงรหัสวรรณยุกต์และสระของฟอนต์ไทยยุคเก่าใน Private Use Area (U+F700..=U+F71A) เป็น Unicode มาตรฐานตอนค้นหาและคัดลอกข้อความ
- **ฟอนต์สำรอง CJK และชื่อเอกสารหลายภาษา:** ชื่อภาษาญี่ปุ่นและจีนในเมนูภาษาไม่แสดงเป็นกล่อง และรองรับการบันทึก/เปิดชื่อเอกสาร (Document Title) ภาษาไทย จีน ญี่ปุ่น อย่างสมบูรณ์ (`doc_title_names.rs`)
- **ระบบวันที่ พ.ศ. และอินเทอร์เฟซภาษาไทย 100%:** แสดงปี พ.ศ. ในส่วนติดต่อผู้ใช้ภาษาไทย พร้อมคำแปล UI ภาษาไทยครบถ้วน (`th.tsv` ครอบคลุม 100% ของแคตตาล็อก)

### 7. OCR สำหรับหน้าที่อ่านข้อความไม่ได้ เอกสารราชการ และสแกนความละเอียดต่ำ
- **FR-17 Unusable Text Detection:** ตรวจหน้าที่ข้อความใช้ไม่ได้ (PUA หรือ U+FFFD เกิน 50%) แสดงแถบเตือนสีส้มพร้อมปุ่มลัดไป Recognize Text และการ์ดยืนยันก่อน OCR เฉพาะหน้านั้น
- **Batch OCR:** `pdfcraft-cli batch --ocr-unusable-text` (พารามิเตอร์ `read_unusable_text` ของ `ocr_recognize_files`) OCR เฉพาะหน้าที่ข้อความใช้ไม่ได้ ข้ามหน้าที่ข้อความปกติสมบูรณ์อยู่แล้ว
- **การจัดการตราประทับและเส้นประราชการ (Seal Filtering & Dot-Leader Removal):** กรองตราประทับราชการ (Seal Recovery), รวมกล่องข้อความบรรทัดเดียวกัน (Colinear Merge), และลบเส้นประช่องกรอกแบบฟอร์มก่อนทำ OCR เพื่อลดสัญญาณรบกวน
- **Auto-upscale:** OCR ภาษาไทยจะ render สแกนที่ต่ำกว่า 200 dpi ขึ้นเป็น 200 dpi และเพิ่ม padding แนวตั้ง (บน 30%, ล่าง 18%) กันวรรณยุกต์ถูกตัด (อยู่บน branch `feat/thai-ocr-auto-upscale` รอ merge)

### 8. ตัวติดตั้ง ระบบ Fork Guard และการตรวจคุณภาพ
- **Windows installer v0.9.0:** WiX v5 (`packaging/windows/package.ps1`) ได้ `.msi`, `.zip` แบบพกพา และ `Install-DocCraftTH.bat` ฝังโมเดล OCR ไทยและอังกฤษ ผูกนามสกุล PDF และเมนูคลิกขวา "Edit with PdfCraft"
- `packaging` ค้นหา `craft-fonts` อัตโนมัติตามโครงสร้างโฟลเดอร์ DocCraftTH
- **ระบบกันฟีเจอร์หาย (Fork Guard — `fork/manifest.txt`, `fork/check.sh`):** ตรวจสอบไฟล์และสัญลักษณ์สำคัญของภาษาไทย (`thai_fonts.rs`, `shaping.rs`, `thai_text.rs`, `map_thai_pua`, `era_year`, `th.tsv`, `vendor/epaint`) อัตโนมัติเมื่อซิงก์โค้ดจาก upstream
- **CI ครบทุกแพลตฟอร์ม:** ตรวจ Ubuntu, Windows, ARM64, Wasm, FreeBSD และ Cargo Deny (แก้ไขเทสต์ `redraw_dedupe_work_is_bounded_on_hostile_pages` ให้วัดด้วยปริมาณงานแทนเวลาจริง เพื่อไม่ให้ล้มบนเครื่องรัน FreeBSD x86_64 ที่โหลดสูง)

### 9. ชุดทดสอบ CER บนเอกสารราชการจริง และการประเมินเปรียบเทียบโมเดล (Empirical Testset & Decision Gate)
จัดทำชุดทดสอบมาตรฐานนอกคลังโค้ด (`data/testset/`) จากเอกสารราชการจริง 84 ฉบับที่ไม่ซ้ำกัน (แบ่งเป็น `test` 16 ฉบับ, `train_candidate` 66 ฉบับ, `excluded` 2 ฉบับ) และพัฒนาชุดทดสอบ Rust [`crates/engine/tests/thai_real_docs.rs`](crates/engine/tests/thai_real_docs.rs) ที่สลับโมเดลด้วย `THAI_CANDIDATE_MODELS_DIR` / `PDFCRAFT_MODELS` โดยไม่เขียนทับ `assets/models/`:

| ชุดทดสอบ (Benchmark Set) | โมเดลเดิมในระบบ (Baseline `8b70e6d1…`) | Candidate v1 (`d0329f75…`) | **Candidate v2 (`07626524…`)** | การเปลี่ยนแปลง (v2 เทียบ v1) | สรุปผลทางเทคนิค |
| :--- | :---: | :---: | :---: | :---: | :--- |
| **1. Synthetic 5 Sentences** (ประโยคมาตรฐาน) | **`0.0136`** | `0.0853` | **`0.0174`** | 🟢 **ลดลง -6.79%** | แก้ปัญหา Catastrophic Forgetting สำเร็จ |
| **2. Set A Recognizer-Only** (787 บรรทัดราชการ) | `0.2260` | `0.1932` | **`0.1915`** | 🟢 **ลดลง -0.17%** | 🏆 **Candidate v2 ชนะเลิศ** (ดีกว่า Baseline -3.45%) |
| **3. Set A End-to-End** (สแกนสังเคราะห์ 27 หน้า) | **`0.3358`** | `0.3788` | **`0.3619`** | 🟢 **ดีขึ้น +1.69%** | Baseline ทนทานต่อ Detector Noise มากกว่า |
| **4. Set B Real-Scan Lines** (สแกนจริง 32 บรรทัดตรวจรับรอง) | **`0.1431`** | `0.2086` | **`0.1952`** | 🟢 **ดีขึ้น +1.34%** | ผ่าน Human Audit Gate 21.3%; Baseline แม่นยำกว่าบนสแกนจริง |
| **คำตัดสิน (Decision Gate Ruling)** | **คงไว้ใน `assets/models/`** | ปฏิเสธ (Rejected) | **ปฏิเสธการสลับ** | ดีขึ้นทุกมิติชัดเจน | **คง Baseline เป็นโมเดลหลักใน Production** |

### 10. ไปป์ไลน์ชุดข้อมูลฝึกฝนรอบใหม่ (Diverse Training Dataset v2 — 24,786 ภาพ)
พัฒนาไปป์ไลน์สร้างชุดข้อมูลฝึกฝน (`tools/dataset_builder/`) เพื่อแก้ปัญหา Catastrophic Forgetting และเพิ่มความหลากหลายของฟอนต์:
- **แหล่งข้อมูลผสมผสาน:** สกัดบรรทัดจากเอกสารราชการจริง 66 ฉบับ ผสานกับประโยคภาษาไทยทั่วไป (Data Replay)
- **Multi-Font Rendering (5 ตระกูลฟอนต์):** TH Sarabun New (40%), Angsana New (15%), Cordia New (15%), Noto Sans Thai (20%), MiSans Thai (10%)
- **Robust Data Augmentation:** จำลอง Bounding Box Jittering (±4–10%), การเอียงภาพ Skew (±1.2°), Gaussian Blur, Noise และ JPEG Compression Artifacts (Q68–90)
- **พจนานุกรมอักขระไทย 524 ตัว (`dict-th.txt`):** รองรับอักษรไทย บาลี/สันสกฤต เลขไทย/อารบิก และสัญลักษณ์ทางราชการครบถ้วน
- **Colab Fine-Tuning Pipeline:** สมุดบันทึก `Thai_OCR_FineTune_DocCraftTH (1).ipynb` ปรับจูนสำหรับ PP-OCRv5 Mobile Recognizer ด้วย Cosine Decay Learning Rate `1e-4`, Epoch 15, Batch Size 64 พร้อมสคริปต์แปลงเป็น ONNX (Opset 11) และ `.rten` อัตโนมัติ

---

## ⚡ คำสั่งใช้งานเบื้องต้น (Quick Start)

### 1. Build และ Compile โปรแกรม
```powershell
powershell -ExecutionPolicy Bypass -File .\build_release.ps1
```

### 2. รันคำสั่ง OCR สร้าง Searchable PDF
```powershell
# กำหนดโฟลเดอร์โมเดลภาษาไทย
$env:PDFCRAFT_MODELS = "K:\data\thai-ocr-eval\bin\models"

# รันคำสั่ง OCR แปลงไฟล์เอกสารสแกน
pdfcraft-cli batch "input.pdf" --steps ocr --language th --dpi 300 --out-dir "out/"
```

### 3. รันชุดทดสอบประเมินผล CER เอกสารราชการจริง (Set A & Set B Benchmark)
```powershell
# กำหนดโฟลเดอร์ชุดทดสอบและโมเดลผู้สมัคร
$env:THAI_TESTSET_DIR = "K:\src\DocCraftTH\data\testset"
$env:THAI_CANDIDATE_MODELS_DIR = "K:\src\DocCraftTH\models\finetuned"

# รันเปรียบเทียบ Set A (Recognizer-only & End-to-End) และ 5 ประโยคมาตรฐาน
cargo test -p pdfcraft-engine --test thai_real_docs compare_models_on_real_corpus -j 2 -- --nocapture

# รันประเมินผล Set B (บรรทัดสแกนจริงที่ผ่านการตรวจรับรอง Human Audit Gate)
cargo test -p pdfcraft-engine --test thai_real_docs test_eval_set_b_real_scan_lines -j 2 -- --nocapture
```

---

## ⚖️ สัญญาอนุญาตและลิขสิทธิ์ (License & Notices)

- ซอร์สโค้ดของโปรแกรมหลักยังคงอยู่ภายใต้สัญญาอนุญาตแบบ Dual License: [MIT](https://opensource.org/licenses/MIT) หรือ [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0) ตามต้นฉบับดั้งเดิมของ ArtCraft Team
- เครื่องหมายการค้า โลโก้ และชื่อ "PdfCraft" / "ArtCraft" เป็นกรรมสิทธิ์ของ ArtCraft Team โครงการนี้อ้างอิงถึงเฉพาะในเชิงเทคนิคเพื่อระบุแหล่งที่มาและประวัติการพัฒนาเท่านั้น
- ข้อมูลพจนานุกรมและชุดข้อมูลภาษาไทยเปิดเผยและใช้งานตามสัญญาอนุญาตของราชบัณฑิตยสภา, NECTEC, PyThaiNLP, VISTEC และเจ้าของชุดข้อมูลแต่ละโครงการ
