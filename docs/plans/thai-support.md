# Thai support plan (fork)

Status: spec, 2026-10-16. Fork: GitHub `DocCraftTH`. Scope: this fork only; upstream is `storytold/pdfcraft`, merged in regularly. Implementation plan: `docs/superpowers/plans/2026-10-16-thai-support.md`.

## Decisions (user, 2026-10-16)
- Repository name: `DocCraftTH`. Product/binary names stay `pdfcraft` so upstream merges stay clean.
- Years: interface language Thai → Buddhist Era (พ.ศ., +543). English (en-GB, en-US) and every other language → Common Era (ค.ศ.). A Thai-interface user can still choose another date language and get ค.ศ.

## Goal
1. Thai file names, PDF titles and other Thai strings in the interface never render as tofu (□).
2. A full Thai interface language (`th`, "ไทย") in Preferences → Interface language.
3. Thai OCR: Scan & OCR ▸ Recognize text (single file and batch, `ocr_recognize` / `ocr_recognize_files`) reads Thai and writes an invisible text layer that keeps Thai characters, so scanned Thai PDFs become searchable and copyable. Quality gate: mean CER ≤ 5 % on synthetic 300 dpi pages. Today pdfcraft-ocr reads Latin only, and its text layer is Helvetica/WinAnsi, which would save Thai as `?`.

## What exists today (verified in code)
- `pdfcraft_fonts::ui_thai_fonts()` / `document_thai_font()` (`crates/fonts/src/craft.rs:70`) pull a `Thai` face from craft-fonts, and `theme.rs:391` appends it to every UI font family. **Only when the build sets `CRAFT_FONTS_DIR` and that directory has a Thai face.**
- `shape_thai` (harfrust) already places stacked vowels/tone marks for Add text in PDFs.
- `system_fonts.rs` loads an *installed* fallback for Devanagari, Telugu, Sinhala and Tamil (`*_fallback()`, wired in `theme.rs:~255`) — **there is no Thai one.** So a normal local build (no craft-fonts) shows Thai file names and titles as tofu. This is the root cause of goal 1.
- No `th.tsv` catalog; 24 languages are registered in `crates/ui-egui/src/i18n/mod.rs:~190–212` (`LangInfo`).
- Template to copy: the Vietnamese PR `db5b9b43` (#1712) — `vi.tsv`, `LangInfo`, locale detection, coverage tests, `engine/src/dates.rs` month names, `ATTRIBUTION.toml` hash, docs.
- `crates/edit/src/linebreak.rs:9` breaks Thai only at spaces (affects PDF text editing, not the menu).

## Global constraints
- Upstream sync: prefer **new files** over edits to contested files (`i18n/mod.rs`, `theme.rs`, catalogues). Where an edit is unavoidable, keep it to a few lines so merges stay trivial.
- AGENTS.md §1: no committed font files; Thai faces come from craft-fonts (`CRAFT_FONTS_DIR`) or from fonts installed on the machine, read at runtime only. Every new `.tsv` needs an `ATTRIBUTION.toml` entry with its SHA-256.
- Translations are clean-room: written from the meaning of the English text, never copied from another product's string tables.
- Never panic; scoped gates per crate: `cargo fmt --check`, `cargo clippy -p pdfcraft-ui-egui -j 2 --all-targets -- -D warnings`, `cargo test -p pdfcraft-ui-egui -j 2`, `cargo xtask assets`, `cargo xtask layers`.

## Phase 0 — fork setup (needs the user)
- Create the fork on GitHub under the new name; set remotes: `origin` → fork, `upstream` → `storytold/pdfcraft`.
- Restore `CLAUDE.md` to upstream; move fork rules (this goal, upstream-sync policy, no PRs upstream) into `CLAUDE.local.md`, listed in `.git/info/exclude`.

## Phase 1 — no tofu for Thai strings (small, ship first)
**Task 1.1 Installed Thai fallback face.**
- Test first (`crates/ui-egui/tests/it/fonts.rs`): `system_fallback_covers_thai` modelled on `system_fallback_covers_telugu` — the embedded-only definitions never name `SYSTEM_THAI_FALLBACK`; the installed ones contain it exactly once per family; `has_glyphs` is true for `"ไฟล์ภาษาไทย น้ำใจ ปฏิบัติ"` (stacked marks, ฏ/ฐ descenders); layout widths finite and > 0. Windows and macOS always ship a Thai face, so on those the test must not skip.
- Implement in `system_fonts.rs`: `THAI_PROBE = 'ก'` (U+0E01), `thai_candidates()` — Windows `LeelawUI.ttf`, `leelawad.ttf`, `tahoma.ttf`; macOS `/System/Library/Fonts/Supplemental/Thonburi.ttc`, `Ayuthaya.ttf`, `Silom.ttf`; Linux Noto Sans Thai / Noto Sans Thai UI / TLWG (Loma, Garuda, Sarabun) paths via `unix_fonts`; `thai_fallback()`.
- Wire it in `theme.rs` next to `SYSTEM_TAMIL_FALLBACK` (one line in the list plus the constant).
- Expected: test fails before the wiring (fallback missing), passes after.

**Task 1.2 Check every place Thai names appear.** With a synthetic PDF whose file name and `/Title` are Thai (`ทดสอบ เอกสาร.pdf`, Title `รายงานประจำปี ๒๕๖๙`): tab strip, recent files, window title, Properties, bookmarks, search results, comments. Use `pdfcraft --control` + `pdfcraft-cli ui screenshot`. Record any place that still boxes or misplaces marks; each becomes a test + fix.

**Task 1.3 Web build.** The web build has no installed fonts, so Thai needs a craft-fonts `Thai` face (Noto Sans Thai, OFL). Check whether `storytold/craft-fonts` has one; if not, the fork's build uses a local craft-fonts checkout that adds Noto Sans Thai with its attribution. Document in `CLAUDE.local.md`.

## Phase 2 — Thai interface language
**Task 2.1 Register `th`.** `LangInfo { code: "th", name: "ไทย", source: include_str!("th.tsv"), plural: plural_none, rtl: false, .. }`; locale detection for `th`, `th_TH`, `th-TH.UTF-8`; `ui.set language th`. Tests (new file `i18n/th_tests.rs`, one `mod` line in `mod.rs`): registration, normalisation, preference round-trip.

**Task 2.2 Catalog `th.tsv`.** Start from the `vi.tsv` key set (~2,600 entries, `context\tEnglish\tThai`), translate in batches by area like #1712: (a) menus and shell chrome, (b) file/organize/pages, (c) comments/markup/view, (d) forms/signatures/certificates, (e) accessibility/preflight/engine sentences, (f) contexts, keys, `@plural` (one form). Coverage tests copied from the Vietnamese ones (commands/tools, `tl!` literals, bundled keys, credits). Update `ATTRIBUTION.toml` hash after each batch.
- Terminology to fix up front (glossary in the `th.tsv` header): ไฟล์, เปิด, บันทึก, บันทึกเป็น, ส่งออก, หน้า, คำอธิบายประกอบ/ความคิดเห็น, ลายเซ็น, ฟอร์ม, ปิดทับข้อความ (redact, permanent), บุ๊กมาร์ก.

**Task 2.3 Dates.** `engine/src/dates.rs`: Thai month names (มกราคม…ธันวาคม, short ม.ค.…ธ.ค.) and พ.ศ. years for `th` (see Decisions). PDF-internal dates (`/CreationDate`, `/M`) stay machine format (CE); only displayed and stamped text uses พ.ศ.

**Task 2.4 Long labels.** Thai has no spaces between words, so egui can't wrap long Thai labels. Check tooltips, Home cards and dialogs at 1280×800; shorten wording first, add a Thai word-break pass only if wording can't fix it.

## Phase 3 — later (not in this plan)
- Thai line breaking for PDF text editing (`crates/edit/src/linebreak.rs`, dictionary-based).
- (Thai OCR moved into scope: goal 3, plan Part B.)

## Review focus
Mark stacking (ไม่ – ที่ – ปั้น – ฤๅ), Thai digits (๐–๙) in titles, very long file names with no spaces, and a build without craft-fonts on each OS.
