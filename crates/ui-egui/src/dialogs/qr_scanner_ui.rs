//! QR code and barcode scanner dialog for PDF pages.

use pdfcraft_engine::qr::{scan_page_qr, QrCodeInfo};

use crate::PdfCraftApp;

#[derive(Default)]
pub(crate) struct QrScannerDraft {
    pub(crate) last_page: Option<usize>,
    pub(crate) results: Option<Result<Vec<QrCodeInfo>, String>>,
}

impl QrScannerDraft {
    pub(crate) fn reset(&mut self) {
        self.last_page = None;
        self.results = None;
    }
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> bool {
    ui.heading(tl!("QR Code Scanner"));
    ui.add_space(4.0);

    let Some((view, id)) = app.active_ids() else {
        ui.label(tl!("No active document"));
        return true;
    };
    let Some(doc) = app.session.get(id) else {
        return true;
    };

    let current_page = app.views[view].current;
    let draft = &mut app.qr_scanner_draft;

    // Trigger scan if not scanned for current page yet
    if draft.last_page != Some(current_page) {
        draft.last_page = Some(current_page);
        let src = doc.export_source();
        let res = scan_page_qr(src.bytes, src.config, current_page);
        draft.results = Some(res);
    }

    ui.label(format!("{} {}", tl!("Page"), current_page + 1));
    ui.separator();

    let mut close = false;

    match &draft.results {
        None => {
            ui.spinner();
            ui.label(tl!("Scanning page for QR codes..."));
        }
        Some(Err(err)) => {
            ui.colored_label(egui::Color32::RED, format!("Scan error: {err}"));
        }
        Some(Ok(items)) if items.is_empty() => {
            ui.add_space(8.0);
            ui.label("ℹ️ ไม่พบคิวอาร์โค้ดในหน้านี้ (No QR Code detected on this page)");
            ui.add_space(8.0);
        }
        Some(Ok(items)) => {
            ui.label(format!("พบคิวอาร์โค้ด {} รายการ:", items.len()));
            ui.add_space(6.0);

            for (idx, item) in items.iter().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(format!("#{}:", idx + 1));
                        let is_url = item.content.starts_with("http://") || item.content.starts_with("https://");
                        if is_url {
                            ui.colored_label(egui::Color32::from_rgb(0, 160, 255), "🔗 Link / URL");
                        } else {
                            ui.colored_label(egui::Color32::GRAY, "📄 ข้อความ (Text)");
                        }
                    });

                    ui.add_space(2.0);

                    let mut text = item.content.clone();
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_rows(3)
                            .desired_width(ui.available_width())
                            .font(egui::TextStyle::Monospace),
                    );

                    ui.horizontal(|ui| {
                        if ui.button("📋 คัดลอก (Copy)").clicked() {
                            ui.ctx().copy_text(item.content.clone());
                        }

                        let is_url = item.content.starts_with("http://") || item.content.starts_with("https://");
                        if is_url {
                            if ui.button("🌐 เปิดลิงก์ (Open URL)").clicked() {
                                ui.ctx().open_url(egui::OpenUrl::same_tab(&item.content));
                            }
                        }
                    });
                });
                ui.add_space(4.0);
            }
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("🔄 สแกนหน้านี้ใหม่ (Rescan)").clicked() {
            app.qr_scanner_draft.last_page = None;
        }
        if ui.button(tl!("Close")).clicked() {
            close = true;
        }
    });

    close
}
