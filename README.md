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

โครงการนี้ได้ทำการปรับปรุงและพัฒนาเพิ่มเติมใน 5 มิติหลัก:

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

---

## ⚖️ สัญญาอนุญาตและลิขสิทธิ์ (License & Notices)

- ซอร์สโค้ดของโปรแกรมหลักยังคงอยู่ภายใต้สัญญาอนุญาตแบบ Dual License: [MIT](https://opensource.org/licenses/MIT) หรือ [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0) ตามต้นฉบับดั้งเดิมของ ArtCraft Team
- เครื่องหมายการค้า โลโก้ และชื่อ "PdfCraft" / "ArtCraft" เป็นกรรมสิทธิ์ของ ArtCraft Team โครงการนี้อ้างอิงถึงเฉพาะในเชิงเทคนิคเพื่อระบุแหล่งที่มาและประวัติการพัฒนาเท่านั้น
- ข้อมูลพจนานุกรมและชุดข้อมูลภาษาไทยเปิดเผยและใช้งานตามสัญญาอนุญาตของราชบัณฑิตยสภา, NECTEC, PyThaiNLP, VISTEC และเจ้าของชุดข้อมูลแต่ละโครงการ
