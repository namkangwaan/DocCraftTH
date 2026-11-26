//! JavaScript values from the host as user-safe text (no `Debug` dumps).

#[cfg(target_arch = "wasm32")]
pub(crate) fn js_error_text(e: &wasm_bindgen::JsValue) -> String {
    e.as_string()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            js_sys::Reflect::get(e, &wasm_bindgen::JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
                .filter(|m| !m.is_empty())
        })
        .unwrap_or_else(|| crate::i18n::t("JavaScript error").to_string())
}