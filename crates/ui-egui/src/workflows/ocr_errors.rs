//! User-facing OCR failure text at the UI boundary (typed [`pdfcraft_engine::ocr::OcrError`] where
//! possible, careful string matching elsewhere).

use pdfcraft_engine::ocr::OcrError;

/// Catalog keys for OCR verify notices (also used as internal `Err` text when the UI shows them).
pub const VERIFY_PAGE_GONE: &str = "The page is gone";
pub const VERIFY_REGION_OUTSIDE: &str = "The selected region is empty or outside the page";
pub const VERIFY_DOC_CLOSED: &str = "The document is no longer open";

/// Catalog message for a missing language model — same placeholder pattern as
/// [`crate::i18n::th_tests::thai_names_the_missing_language_model`] (FR-05).
pub fn missing_language_model(language: &str) -> String {
    let key = OcrError::NoLanguageModel("{language}".into()).to_string();
    crate::i18n::fmt(&crate::i18n::t(&key), &[("language", language)])
}

/// [`OcrError`] variants the catalog knows become translated notices; others keep [`Display`].
pub fn ocr_user_message(e: &OcrError) -> String {
    match e {
        OcrError::NoModels => crate::i18n::t(&e.to_string()).to_string(),
        OcrError::NoLanguageModel(language) => missing_language_model(language),
        _ => e.to_string(),
    }
}

/// When the pipeline only has a [`String`] (engine APIs, panics wrapped as text), recover the
/// known OCR catalog messages without broad pattern matching.
pub fn ocr_notice_message(raw: &str) -> String {
    if raw == pdfcraft_engine::EditError::Cancelled.to_string() {
        return crate::i18n::t("Cancelled").to_string();
    }
    if raw == OcrError::NoModels.to_string() {
        return ocr_user_message(&OcrError::NoModels);
    }
    if let Some(language) = parse_missing_language_model(raw) {
        return missing_language_model(&language);
    }
    if matches!(raw, VERIFY_PAGE_GONE | "the page is gone") {
        return crate::i18n::t(VERIFY_PAGE_GONE).to_string();
    }
    if matches!(raw, VERIFY_REGION_OUTSIDE | "the selected region is empty or outside the page") {
        return crate::i18n::t(VERIFY_REGION_OUTSIDE).to_string();
    }
    if matches!(raw, VERIFY_DOC_CLOSED | "the document is no longer open") {
        return crate::i18n::t(VERIFY_DOC_CLOSED).to_string();
    }
    raw.to_string()
}

fn parse_missing_language_model(raw: &str) -> Option<String> {
    let template = OcrError::NoLanguageModel("{language}".into()).to_string();
    let (prefix, suffix) = template.split_once("{language}")?;
    let language = raw.strip_prefix(prefix)?.strip_suffix(suffix)?;
    if language.is_empty() { None } else { Some(language.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{fmt, tr, Lang};
    use pdfcraft_engine::ocr::OcrError;

    /// FR-05: catalog key is the engine wording; `{language}` is filled for the notice.
    #[test]
    fn no_language_model_uses_catalog_key() {
        let msg = ocr_user_message(&OcrError::NoLanguageModel("Thai".into()));
        assert_eq!(msg, missing_language_model("Thai"));
        assert!(msg.contains("Thai") && msg.contains("PDFCRAFT_MODELS"), "{msg}");
    }

    #[test]
    fn thai_missing_language_matches_fr05_pattern() {
        let th = Lang::from_code("th").expect("th registered");
        let key = OcrError::NoLanguageModel("{language}".into()).to_string();
        let expect = fmt(tr(th, &key), &[("language", "ไทย")]);
        crate::i18n::set_current(th);
        let got = ocr_user_message(&OcrError::NoLanguageModel("ไทย".into()));
        assert_eq!(got, expect, "{got}");
        assert!(got.contains("ไทย") && got.contains("PDFCRAFT_MODELS") && !got.is_ascii(), "{got}");
        assert!(!got.contains("not installed"), "{got}");
    }

    #[test]
    fn notice_message_maps_collapsed_model_errors() {
        let models = OcrError::NoModels.to_string();
        assert_eq!(ocr_notice_message(&models), ocr_user_message(&OcrError::NoModels));
        let thai = OcrError::NoLanguageModel("th".into()).to_string();
        assert_eq!(ocr_notice_message(&thai), ocr_user_message(&OcrError::NoLanguageModel("th".into())));
        assert_eq!(ocr_notice_message("cancelled"), crate::i18n::t("Cancelled").to_string());
    }

    #[test]
    fn notice_message_maps_verify_literals() {
        assert_eq!(ocr_notice_message(VERIFY_PAGE_GONE), crate::i18n::t(VERIFY_PAGE_GONE).to_string());
        assert_eq!(ocr_notice_message("the page is gone"), crate::i18n::t(VERIFY_PAGE_GONE).to_string());
        assert_eq!(ocr_notice_message(VERIFY_REGION_OUTSIDE), crate::i18n::t(VERIFY_REGION_OUTSIDE).to_string());
        assert_eq!(ocr_notice_message(VERIFY_DOC_CLOSED), crate::i18n::t(VERIFY_DOC_CLOSED).to_string());
        assert_eq!(
            ocr_notice_message("the document is no longer open"),
            crate::i18n::t(VERIFY_DOC_CLOSED).to_string()
        );
    }
}