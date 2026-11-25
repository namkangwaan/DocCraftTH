//! The interface fonts with and without the optional craft-fonts build input (`CRAFT_FONTS_DIR`).
//! Installed-face search and its order are the framework's `OnDemandFonts`; these tests guard
//! PdfCraft's own catalogs against its `UiFonts` input.

use egui::epaint::text::{FontDefinitions, Fonts, TextOptions};
use egui::{Color32, FontFamily, FontId};
use pdfcraft_ui_egui::fonts::ui_fonts;
use pdfcraft_ui_egui::i18n::Lang;

const JAPANESE: &str = "日本語の文字";
const CHINESE: &str = "简体中文欢迎";
const ARABIC: &str = "واحد اثنين";
const TELUGU: &str = "తెలుగు ఫైల్";
const DEVANAGARI: &str = "हिंदी पाठ सम्पादन";

/// The embedded craft-i18n catalogs `code` reads (the shared one and PdfCraft's), one text; empty
/// in a build without them.
fn catalog(code: &str) -> String {
    Lang::from_code(code).map(|l| l.catalog_texts().collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// The interface stack for the language `code` (English when it is not embedded), as one set of
/// definitions: each family's chain, including the faces asked after the system fonts.
fn definitions(code: &str) -> FontDefinitions {
    ui_fonts(Lang::from_code(code).unwrap_or(Lang::EN)).stack().flat_definitions()
}

fn families() -> Vec<FontId> {
    vec![FontId::proportional(13.0), FontId::monospace(13.0)]
}

/// The bundled faces, with no craft-fonts and no installed system face: text symbols must keep
/// working with `PDFCRAFT_SYSTEM_FONTS=0`.
#[test]
fn bundled_monochrome_symbols_have_glyphs() {
    let mut fonts = Fonts::new(TextOptions::default(), definitions("en"));
    for id in families() {
        for text in ["✔", "▲", "▼"] {
            assert!(fonts.has_glyphs(&id, text), "{id:?} lacks {text} in the interface stack");
        }
    }
}

/// Lays `text` out in every interface family and returns each galley's width.
fn layout_widths(fonts: &mut Fonts, text: &str) -> Vec<f32> {
    let mut view = fonts.with_pixels_per_point(2.0);
    families().into_iter().map(|id| view.layout_no_wrap(text.to_owned(), id, Color32::BLACK).size().x).collect()
}

/// Built with craft-fonts, Japanese text renders with real glyphs (no tofu) in every family,
/// from a craft-fonts face placed after the app's own fonts.
#[test]
fn japanese_ui_text_uses_craft_fonts() {
    if pdfcraft_fonts::ui_japanese_fonts().is_empty() {
        eprintln!("skipping japanese_ui_text_uses_craft_fonts: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
        return;
    }
    let defs = definitions("ja");
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_jp = stack.iter().position(|n| n.starts_with("BIZ UDPGothic")).expect("BIZ UDPGothic is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_jp, "{family:?}: {stack:?}");
        assert!(stack[first_jp..].iter().all(|n| pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.name() == *n)), "{stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, JAPANESE), "{id:?} lacks {JAPANESE}");
    }
    // Real glyphs are about a full em wide each; tofu boxes and missing glyphs are not.
    for w in layout_widths(&mut fonts, JAPANESE) {
        assert!(w > 13.0 * 0.8 * JAPANESE.chars().count() as f32, "{w}");
    }
}

/// Chinese mode orders the CJK fallback with the `Hans` group first, so one line never
/// mixes faces with different vertical metrics. Without a `Hans` face in the build input the
/// order part still holds; glyph coverage follows the craft-fonts checkout.
#[test]
fn chinese_ui_text_prefers_the_chinese_face() {
    if pdfcraft_fonts::ui_chinese_fonts().is_empty() {
        eprintln!("skipping chinese_ui_text_prefers_the_chinese_face: no Hans face bundled (set CRAFT_FONTS_DIR with a Chinese face to run it)");
        return;
    }
    let defs = definitions("zh-hans");
    let hans: Vec<String> = pdfcraft_fonts::ui_chinese_fonts().iter().map(|f| f.name()).collect();
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_zh = stack.iter().position(|n| hans.iter().any(|h| h == n)).expect("a Hans face is a fallback");
        let first_ja = stack.iter().position(|n| n.starts_with("BIZ UDPGothic")).expect("BIZ UDPGothic is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_zh && first_zh < first_ja, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, CHINESE), "{id:?} lacks {CHINESE}");
    }
    for w in layout_widths(&mut fonts, CHINESE) {
        assert!(w > 13.0 * 0.8 * CHINESE.chars().count() as f32, "{w}");
    }
}

/// Built with a craft-fonts Arabic face, Arabic text has real glyphs in every family, from a
/// face placed after the app's own fonts.
#[test]
fn arabic_ui_text_uses_craft_fonts() {
    let arabic: Vec<String> = pdfcraft_fonts::ui_arabic_fonts().iter().map(|f| f.name()).collect();
    if arabic.is_empty() {
        eprintln!("skipping arabic_ui_text_uses_craft_fonts: no Arab face bundled (set CRAFT_FONTS_DIR with an Arabic face to run it)");
        return;
    }
    let defs = definitions("ar");
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_ar = stack.iter().position(|n| arabic.contains(n)).expect("an Arab face is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_ar, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, ARABIC), "{id:?} lacks {ARABIC}");
    }
}

/// Built with a craft-fonts Telugu face, Telugu text has real glyphs in every family, from a
/// face placed after the app's own fonts, and is shaped: the conjunct క్ష is narrower than క్
/// and ష held apart by a zero-width non-joiner.
#[test]
fn telugu_ui_text_uses_craft_fonts() {
    let telugu: Vec<String> = pdfcraft_fonts::CRAFT_FONTS.iter().filter(|f| f.has_script("Telu")).map(|f| f.name()).collect();
    if telugu.is_empty() {
        eprintln!("skipping telugu_ui_text_uses_craft_fonts: no Telu face bundled (set CRAFT_FONTS_DIR with a Telugu face to run it)");
        return;
    }
    let defs = definitions("te");
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first_te = stack.iter().position(|n| telugu.contains(n)).expect("a Telu face is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first_te, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, TELUGU), "{id:?} lacks {TELUGU}");
    }
    let joined = layout_widths(&mut fonts, "క్ష");
    let apart = layout_widths(&mut fonts, "క్\u{200c}ష");
    for (j, a) in joined.iter().zip(&apart) {
        assert!(j < a, "క్ష is one conjunct, narrower than క్‌ష: {j} vs {a}");
    }
}

/// Built with craft-fonts, Hindi text renders with real glyphs (no tofu) in every family: a
/// `Deva` face is a fallback after the app's own fonts. Without craft-fonts it is the installed
/// face's job (the framework's on-demand search), and the web build has neither.
#[test]
fn devanagari_ui_text_uses_craft_fonts() {
    if pdfcraft_fonts::ui_devanagari_fonts().is_empty() {
        eprintln!("skipping devanagari_ui_text_uses_craft_fonts: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
        return;
    }
    let defs = definitions("hi");
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let stack = &defs.families[&family];
        let first =
            stack.iter().position(|n| pdfcraft_fonts::ui_devanagari_fonts().iter().any(|f| f.name() == *n)).expect("a Deva face is a fallback");
        let own = stack.iter().position(|n| n == "Inter" || n == "JetBrainsMono").expect("the app's own font");
        assert!(own < first, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, DEVANAGARI), "{id:?} lacks {DEVANAGARI}");
    }
    assert!(layout_widths(&mut fonts, DEVANAGARI).iter().all(|w| w.is_finite() && *w > 0.0));
}

/// Without craft-fonts the interface fonts still install and lay out any text (Japanese falls
/// back to egui's replacement glyph) without panicking; Latin text is unaffected.
#[test]
fn ui_fonts_work_without_craft_fonts() {
    let mut fonts = Fonts::new(TextOptions::default(), definitions("en"));
    for id in families() {
        assert!(fonts.has_glyphs(&id, "PdfCraft"), "{id:?}");
        assert_eq!(fonts.has_glyphs(&id, JAPANESE), !pdfcraft_fonts::ui_japanese_fonts().is_empty(), "{id:?}");
    }
    assert!(layout_widths(&mut fonts, JAPANESE).iter().all(|w| w.is_finite() && *w > 0.0));
    assert!(layout_widths(&mut fonts, "PdfCraft").iter().all(|w| *w > 20.0));
    let ctx = egui::Context::default();
    ui_fonts(Lang::EN).install(&ctx);
}

/// Latin- and Cyrillic-script catalogs (Czech, Brazilian Portuguese, German, Spanish, French, Russian, Bulgarian, Kazakh, Hungarian, Ukrainian, Italian, Slovene, Lithuanian, Greek, Turkish, Indonesian, Vietnamese, Albanian, Catalan) are drawn entirely by the app's own faces
/// (Inter, JetBrains Mono with Inter fallback): no letter falls through to egui's defaults or a CJK fallback. (egui's
/// `has_glyphs` can't answer this: with only the primary face it is also the replacement face.)
#[test]
fn primary_ui_fonts_cover_latin_and_cyrillic_catalogs() {
    if crate::support::skip_without_catalogs("primary_ui_fonts_cover_latin_and_cyrillic_catalogs") {
        return;
    }
    use skrifa::MetadataProvider as _;
    let defs = definitions("en");
    assert_eq!(&defs.families[&FontFamily::Monospace][..2], ["JetBrainsMono", "Inter"]);
    let fallback = &defs.font_data["Inter"];
    let fallback_font = skrifa::FontRef::from_index(fallback.font.as_ref().as_ref(), fallback.index).unwrap();
    let fallback_charmap = fallback_font.charmap();
    for (code, catalog) in [
        ("cs", &catalog("cs")),
        ("el", &catalog("el")),
        ("pt-br", &catalog("pt-br")),
        ("de", &catalog("de")),
        ("es", &catalog("es")),
        ("fr", &catalog("fr")),
        ("ru", &catalog("ru")),
        ("kk", &catalog("kk")),
        ("bg", &catalog("bg")),
        ("hu", &catalog("hu")),
        ("uk", &catalog("uk")),
        ("it", &catalog("it")),
        ("tr", &catalog("tr")),
        ("sl", &catalog("sl")),
        ("lt", &catalog("lt")),
        ("id", &catalog("id")),
        ("vi", &catalog("vi")),
        ("fi", &catalog("fi")),
        ("sq", &catalog("sq")),
        ("ca", &catalog("ca")),
        ("ro", &catalog("ro")),
        ("nb", &catalog("nb")),
    ] {
        // Vietnamese stacks its diacritics: the precomposed letters and the base letters they
        // combine with, including `đ` and the tone marks `ạ ả ấ ầ ẩ ẫ ậ ắ ằ ẳ ẵ ặ ế ề ể ễ ệ`.
        let mut text = String::from(
            "áčďéěíňóřšťúůýžÁČĎÉĚÍŇÓŘŠŤÚŮÝŽãõçâêôàÃÕÇÂÊÔÀñÑüÜäÄöÖőűßæøåÆØÅ„“¿¡«»…ґҐєЄіІїЇәғқңөұүһӘҒҚҢӨҰҮҺđĐạảấầẩẫậắằẳẵặếềểễệọỏốồổỗộớờởỡợụủứừửữựỳỵỷỹ",
        );
        for line in catalog.lines().filter(|l| !l.starts_with('#')) {
            let mut columns = line.split('\t').skip(1);
            if let (Some(source), Some(translation)) = (columns.next(), columns.next()) {
                // Every letter is checked. A symbol the row's English text has too (the menu path
                // arrow `▸`, say) is drawn however English draws it, so it is not this catalog's
                // to cover.
                text.extend(translation.chars().filter(|c| !c.is_whitespace() && (c.is_alphanumeric() || !source.contains(*c))));
            }
        }
        for name in ["Inter", "Inter-Medium", "Inter-SemiBold", "JetBrainsMono"] {
            let data = &defs.font_data[name];
            let font = skrifa::FontRef::from_index(data.font.as_ref().as_ref(), data.index).unwrap();
            let charmap = font.charmap();
            let mut missing: Vec<char> =
                text.chars().filter(|c| charmap.map(*c).is_none() && (name != "JetBrainsMono" || fallback_charmap.map(*c).is_none())).collect();
            missing.sort_unstable();
            missing.dedup();
            assert!(missing.is_empty(), "{code}: {name} lacks {missing:?}");
        }
    }
}
<<<<<<< ours

/// Thai file names and titles with stacked marks, Thai digits and descenders (ฏ ฐ).
const THAI: &str = "ไฟล์ภาษาไทย น้ำใจ ปฏิบัติ ฐาน ๒๕๖๙ ปั้น ฤๅษี";

/// On a machine with an installed Thai font, the installed definitions include it and Thai text,
/// such as a file name or a document title, has glyphs instead of boxes. Windows (Leelawadee UI,
/// Tahoma) and macOS (Thonburi) always ship one, so there the test never skips.
#[test]
fn system_fallback_covers_thai() {
    assert!(!theme::font_definitions().font_data.contains_key(theme::SYSTEM_THAI_FALLBACK));
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    let off = std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0");
    let always = cfg!(windows) || cfg!(target_os = "macos");
    let embedded = !pdfcraft_fonts::ui_thai_fonts().is_empty();
    if !defs.font_data.contains_key(theme::SYSTEM_THAI_FALLBACK) && (off || !always) {
        eprintln!("skipping system_fallback_covers_thai: no installed Thai fallback font (or PDFCRAFT_SYSTEM_FONTS=0)");
        assert!(defs.families.values().all(|stack| !stack.iter().any(|n| n == theme::SYSTEM_THAI_FALLBACK)));
        return;
    }
    for (family, stack) in &defs.families {
        assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_THAI_FALLBACK).count(), 1, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for (i, id) in families().iter().enumerate() {
        assert!(fonts.has_glyphs(id, THAI), "{id:?} lacks {THAI} (embedded Thai face: {embedded})");
        // Marks have no advance once shaped: stacked marks do not widen their base glyph (M4).
        let w_thi = layout_widths(&mut fonts, "ที่")[i];
        let w_th = layout_widths(&mut fonts, "ท")[i];
        assert!((w_thi - w_th).abs() <= w_th * 0.25, "{id:?}: ที่ ({w_thi}) widens ท ({w_th})");

        let w_nam = layout_widths(&mut fonts, "น้ำ")[i];
        let w_num = layout_widths(&mut fonts, "นำ")[i];
        assert!((w_nam - w_num).abs() <= w_num * 0.25, "{id:?}: น้ำ ({w_nam}) widens นำ ({w_num})");
    }
    assert!(layout_widths(&mut fonts, THAI).iter().all(|w| w.is_finite() && *w > 0.0));
}

/// On a machine with an installed CJK font, the installed definitions include it and CJK
/// language names (日本語, 简体中文, 繁體中文 in Preferences) have glyphs instead of boxes.
#[test]
fn system_fallback_covers_cjk() {
    assert!(!theme::font_definitions().font_data.contains_key(theme::SYSTEM_CJK_FALLBACK));
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    let off = std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0");
    let always = cfg!(windows) || cfg!(target_os = "macos");
    if !defs.font_data.contains_key(theme::SYSTEM_CJK_FALLBACK) && (off || !always) {
        eprintln!("skipping system_fallback_covers_cjk: no installed CJK fallback font (or PDFCRAFT_SYSTEM_FONTS=0)");
        assert!(defs.families.values().all(|stack| !stack.iter().any(|n| n == theme::SYSTEM_CJK_FALLBACK)));
        return;
    }
    for (family, stack) in &defs.families {
        assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_CJK_FALLBACK).count(), 1, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for name in ["日本語", "简体中文", "繁體中文"] {
        for id in families() {
            assert!(fonts.has_glyphs(&id, name), "{id:?} lacks {name}");
        }
        assert!(layout_widths(&mut fonts, name).iter().all(|w| w.is_finite() && *w > 0.0));
    }
}

/// On a machine with an installed Korean font, the installed definitions include it and Hangul has
/// glyphs, so a Korean file name in a tab title is not boxes. macOS always ships Apple SD Gothic
/// Neo and Windows Malgun Gothic, so there the test never skips.
#[test]
fn system_fallback_covers_hangul() {
    const HANGUL: &str = "성가대 찬양곡 목록";
    assert!(!theme::font_definitions().font_data.contains_key(theme::SYSTEM_HANGUL_FALLBACK));
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    let off = std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0");
    if !defs.font_data.contains_key(theme::SYSTEM_HANGUL_FALLBACK) && (off || cfg!(all(unix, not(target_os = "macos")))) {
        eprintln!("skipping system_fallback_covers_hangul: no installed Korean fallback font (or PDFCRAFT_SYSTEM_FONTS=0)");
        assert!(defs.families.values().all(|stack| !stack.iter().any(|n| n == theme::SYSTEM_HANGUL_FALLBACK)));
        return;
    }
    for (family, stack) in &defs.families {
        assert_eq!(stack.iter().filter(|n| *n == theme::SYSTEM_HANGUL_FALLBACK).count(), 1, "{family:?}: {stack:?}");
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        assert!(fonts.has_glyphs(&id, HANGUL), "{id:?} lacks {HANGUL}");
    }
    assert!(layout_widths(&mut fonts, HANGUL).iter().all(|w| w.is_finite() && *w > 0.0));
}
=======
>>>>>>> theirs
