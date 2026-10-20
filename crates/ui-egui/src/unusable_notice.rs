//! The notice shown while browsing a page whose own text is unusable (SRS §5.5, FR-17, N9,
//! S-09): its text can't be searched or copied, as when a legacy Thai font maps its glyphs to
//! private-use code points. The notice only suggests text recognition. Its button opens
//! Recognize Text set to this page, and nothing is read until the user confirms there.

use std::collections::HashSet;
use std::sync::Arc;

use egui::{Align, Layout};
use pdfcraft_engine::DocId;

use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

/// Documents whose notice the user dismissed with "Not now", for this session.
#[derive(Clone, Default)]
struct Dismissed(Arc<HashSet<DocId>>);

fn dismissed_id() -> egui::Id {
    egui::Id::new("ocr-unusable-page-notice-dismissed")
}

impl PdfCraftApp {
    /// Whether the active document's current page has unusable text of its own: `None` until
    /// the page's text has been read (or when there is no document), so a test can wait for it.
    pub fn current_page_text_unusable(&self) -> Option<bool> {
        let (vi, _) = self.active_ids()?;
        let view = self.views.get(vi)?;
        view.page_text(view.current).map(|t| t.is_unusable())
    }

    /// Draw the notice over the top of the document area when the current page's text is
    /// unusable. It steps aside for the offer after a run, a running job and open dialogs, so
    /// no two cards cover each other.
    pub(crate) fn show_unusable_page_notice(&mut self, ctx: &egui::Context) {
        if self.ocr_offer.is_some() || self.ocr_run.is_some() || self.progress_notice.is_some() || self.modal_open() {
            return;
        }
        let Some((vi, doc)) = self.active_ids() else { return };
        if self.current_page_text_unusable() != Some(true) {
            return;
        }
        if ctx.data(|d| d.get_temp::<Dismissed>(dismissed_id())).is_some_and(|d| d.0.contains(&doc)) {
            return;
        }
        let Some(page) = self.views.get(vi).map(|v| v.current) else { return };
        let t = Tokens::get(ctx);
        let area = self.document_rect.filter(|r| r.is_positive()).unwrap_or_else(|| ctx.content_rect());
        let width = 420.0_f32.min(area.width() - 32.0).max(160.0);
        let (mut open, mut later) = (false, false);
        egui::Area::new(egui::Id::new("ocr-unusable-page-notice"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(area.center_top() + egui::vec2(0.0, 16.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(t.card).corner_radius(egui::CornerRadius::same(10)).inner_margin(egui::Margin::same(12)).show(
                    ui,
                    |ui| {
                        ui.set_width(width);
                        ui.label(egui::RichText::new(tl!("This page has unusable text")).color(t.text).strong());
                        ui.label(
                            egui::RichText::new(tl!("It can't be searched or copied. Recognizing text on this page can fix that."))
                                .color(t.text_muted),
                        );
                        ui.add_space(8.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            open = widgets::pill_button(ui, tl!("Recognize text on this page…"), true).clicked();
                            later = widgets::pill_button(ui, tl!("Not now"), false).clicked();
                        });
                    },
                );
            });
        if open {
            self.recognize_unusable_page(page);
        } else if later {
            ctx.data_mut(|d| {
                let set = &mut d.get_temp_mut_or_default::<Dismissed>(dismissed_id()).0;
                Arc::make_mut(set).insert(doc);
            });
        }
    }

    /// The notice's button: Recognize Text set to `page`, with reading over its unusable text
    /// ticked. It only opens the dialog; the user still confirms the run there.
    pub fn recognize_unusable_page(&mut self, page: usize) {
        self.recognize_text_on_page(page);
        self.ocr_draft.read_unusable_text = true;
    }
}
