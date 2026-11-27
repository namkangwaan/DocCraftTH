//! Thai form field font selection, substitution notice, unknown `/DA` preservation, and bulk edit.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{Edit, FieldFont, FieldProps, FieldValue, NewField, Session};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::bulk_fields::SharedFieldPropsDialog;

fn new_app() -> PdfCraftApp {
    use craft::host::{AppInfo, Host, Wake};
    use craft::platform::clipboard::{Clipboard, SystemClipboard};
    use craft::platform::open::FileIntake;
    use craft::platform::watch::FileWatch;
    let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "test" }).build();
    let services = pdfcraft_ui_egui::AppServices::new(
        Wake::none(),
        Clipboard::new(SystemClipboard::new()),
        None,
        None,
        FileIntake::new(Wake::none()).0,
        FileWatch::new(Wake::none()).inline(),
    );
    PdfCraftApp::with_host(std::sync::Arc::new(host), services)
}

fn install_runtime(ctx: &egui::Context) {
    let spec = craft::ui::InstallSpec::new(craft::host::target::ShellInfo::default(), std::sync::Arc::new(craft::window::system::NoWindows));
    craft::ui::shell::embed::install(ctx, spec, &pdfcraft_ui_egui::fonts::ui_fonts(pdfcraft_ui_egui::i18n::Lang::EN))
        .expect("the runtime installs on a fresh context");
}

fn harness_with(name: &'static str, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        install_runtime(&cc.egui_ctx);
        let mut app = new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes(&egui::Context::default(), name, None, bytes).unwrap();
        app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn exec(h: &mut Harness<'static, PdfCraftApp>, cmd: &str) {
    let ctx = h.ctx.clone();
    assert!(h.state_mut().execute(&ctx, cmd), "command {cmd}");
    h.run_steps(3);
}

fn open_props(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let ctx = h.ctx.clone();
    h.state_mut().set_option(&ctx, "dialog", &format!("field-properties:{name}")).unwrap();
    h.run_steps(2);
}

fn tab(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_role_and_label(Role::Tab, label).click();
    h.run_steps(2);
}

fn pick(h: &mut Harness<'static, PdfCraftApp>, label: &str, option: &str) {
    h.get_by_label(label).click();
    h.run_steps(2);
    h.get_all_by_label(option).last().expect("the list offers the option").click();
    h.run_steps(2);
}

/// A PDF of the given objects (numbered from 1).
fn pdf_of(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn font_list_offers_the_bundled_thai_face_and_embeds_it() {
    let sarabun = pdfcraft_fonts::thai_faces::sarabun().expect("CRAFT_FONTS_DIR must provide Sarabun");
    let mut h = harness_with("form.pdf", include_bytes!("data/form.pdf").to_vec());
    exec(&mut h, "form.prepare");
    open_props(&mut h, "city");
    tab(&mut h, "Appearance");
    pick(&mut h, "Font:", "Sarabun");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let l = s.session.get(s.views[0].id).unwrap().field_look("city").unwrap();
    assert_eq!(l.font, FieldFont::Thai(sarabun));
}

#[test]
fn ok_without_touching_font_keeps_an_unknown_da_font() {
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>",
        "<< /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (city) /DA (/THSarabunPSK 10 Tf 0 g) /Rect [40 290 230 314] /P 3 0 R >>",
    ]);

    let mut h = harness_with("dga.pdf", bytes);
    exec(&mut h, "form.prepare");
    open_props(&mut h, "city");
    tab(&mut h, "Appearance");
    // Change another Appearance property (Line Thickness) without touching Font.
    pick(&mut h, "Line Thickness:", "Thick");
    h.get_by_label("OK").click();
    h.run_steps(3);
    {
        let s = h.state();
        let d = s.session.get(s.views[0].id).unwrap();
        assert_eq!(d.field_face("city"), Some(pdfcraft_engine::FieldFace::Unknown("THSarabunPSK".into())));
        assert_eq!(d.field_look("city").unwrap().width, 3.0);
    }
    // Explicitly choosing Courier does replace the font.
    open_props(&mut h, "city");
    tab(&mut h, "Appearance");
    pick(&mut h, "Font:", "Courier");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let d = s.session.get(s.views[0].id).unwrap();
    assert_eq!(d.field_look("city").unwrap().font, FieldFont::Courier);
}

#[test]
fn choosing_a_substituted_face_shows_the_warning_and_stores_sarabun() {
    let sarabun = pdfcraft_fonts::thai_faces::sarabun().expect("CRAFT_FONTS_DIR must provide Sarabun");
    let variable = pdfcraft_fonts::document_thai_font().expect("CRAFT_FONTS_DIR must provide Noto Sans Thai");
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-thai-sub-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut copy = variable.bytes.to_vec();
    copy.extend_from_slice(&7u32.to_le_bytes());
    let file = dir.join("variable.ttf");
    std::fs::write(&file, copy).unwrap();
    let entries = pdfcraft_engine::thai_catalog::thai_entries(&craft::font_catalog::Catalog::of_files(vec![file]));
    let _ = std::fs::remove_dir_all(&dir);
    let sub = entries.iter().find(|e| e.substituted).expect("a substituted Thai face").clone();
    let option_label = format!("{} (shown as Sarabun)", sub.family);

    let mut draft = pdfcraft_ui_egui::prepare::FieldDraft::default();
    draft.look = Some(pdfcraft_engine::FieldLook {
        border: None,
        fill: None,
        width: 1.0,
        style: pdfcraft_engine::BorderStyle::Solid,
        text: [0.0; 3],
        font: FieldFont::Helvetica,
    });
    draft.face = Some(pdfcraft_engine::FieldFace::Standard);
    let mut installed = false;
    let mut h = Harness::builder().with_size(egui::vec2(600.0, 300.0)).build_ui_state(
        move |ui, d: &mut pdfcraft_ui_egui::prepare::FieldDraft| {
            if !installed {
                install_runtime(ui.ctx());
                installed = true;
            }
            pdfcraft_ui_egui::prepare::show_font_row_with_entries(ui, d, &entries);
        },
        draft,
    );
    h.run_steps(2);
    h.get_by_label("Font:").click();
    h.run_steps(2);
    h.get_all_by_label(&option_label).last().expect("substituted face option").click();
    h.run_steps(2);
    assert_eq!(h.state().look.unwrap().font, FieldFont::Thai(sarabun));
    let expected_notice = format!("{} can't be embedded in a form. The field will use Sarabun.", sub.family);
    assert!(h.query_all_by_label_contains(&expected_notice).next().is_some(), "accessibility tree must carry the substitution notice");
}

#[test]
fn bulk_edit_can_set_a_thai_face() {
    let sarabun = pdfcraft_fonts::thai_faces::sarabun().expect("CRAFT_FONTS_DIR must provide Sarabun");
    let mut s = Session::new();
    let bytes = s.create_blank(300.0, 400.0, 1).unwrap();
    let id = s.open("form.pdf", None, bytes, None).unwrap();
    for (name, rect, font) in [("alpha", [40.0, 290.0, 230.0, 314.0], FieldFont::Courier), ("beta", [40.0, 240.0, 230.0, 264.0], FieldFont::Times)] {
        s.apply(id, Edit::AddField { name: Some(name.into()), page: 0, rect, kind: NewField::Text { multiline: false } }).unwrap();
        s.apply(id, Edit::SetFieldValue { name: name.into(), value: FieldValue::Text("สวัสดี".into()) }).unwrap();
        let mut look = s.get(id).unwrap().field_look(name).unwrap();
        look.font = font;
        s.apply(id, Edit::SetFieldProps { name: name.into(), props: Box::new(FieldProps { look: Some(look), ..Default::default() }) }).unwrap();
    }
    let fixture = s.save_bytes(id).unwrap().as_ref().clone();

    let mut h = harness_with("form.pdf", fixture);
    exec(&mut h, "form.prepare");
    {
        let p = &mut h.state_mut().views[0].session.prepare;
        p.selected = Some(("alpha".into(), 0));
        p.also = vec![("beta".into(), 0)];
    }
    exec(&mut h, "form.field.properties");
    assert!(h.state().dialogs.is_open::<SharedFieldPropsDialog>());
    h.get_by_label("Appearance").click();
    h.run_steps(4);
    h.get_by_role_and_label(Role::Group, "Font:").get_by_role_and_label(Role::CheckBox, "Enable").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Group, "Font:").get_by_role(Role::ComboBox).click();
    h.run_steps(2);
    h.get_all_by_label("Sarabun").last().expect("Sarabun option").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    let state = h.state();
    let doc = state.session.get(state.views[0].id).unwrap();
    assert_eq!(doc.field_look("alpha").unwrap().font, FieldFont::Thai(sarabun));
    assert_eq!(doc.field_look("beta").unwrap().font, FieldFont::Thai(sarabun));
}

#[test]
fn substituted_notice_does_not_leak_across_reopen_or_other_field() {
    let (Some(sarabun), Some(variable)) = (pdfcraft_fonts::thai_faces::sarabun(), pdfcraft_fonts::document_thai_font()) else { return };
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-thai-leak-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut copy = variable.bytes.to_vec();
    copy.extend_from_slice(&9u32.to_le_bytes());
    let file = dir.join("variable.ttf");
    std::fs::write(&file, copy).unwrap();
    let entries = pdfcraft_engine::thai_catalog::thai_entries(&craft::font_catalog::Catalog::of_files(vec![file]));
    let _ = std::fs::remove_dir_all(&dir);
    let sub = entries.iter().find(|e| e.substituted).expect("a substituted Thai face").clone();
    let option_label = format!("{} (shown as Sarabun)", sub.family);

    let mut draft1 = pdfcraft_ui_egui::prepare::FieldDraft::default();
    draft1.field = "city".into();
    draft1.look = Some(pdfcraft_engine::FieldLook {
        border: None,
        fill: None,
        width: 1.0,
        style: pdfcraft_engine::BorderStyle::Solid,
        text: [0.0; 3],
        font: FieldFont::Helvetica,
    });
    draft1.face = Some(pdfcraft_engine::FieldFace::Standard);
    let mut installed = false;
    let mut h = Harness::builder().with_size(egui::vec2(600.0, 300.0)).build_ui_state(
        move |ui, d: &mut pdfcraft_ui_egui::prepare::FieldDraft| {
            if !installed {
                install_runtime(ui.ctx());
                installed = true;
            }
            pdfcraft_ui_egui::prepare::show_font_row_with_entries(ui, d, &entries);
        },
        draft1,
    );
    h.run_steps(2);
    h.get_by_label("Font:").click();
    h.run_steps(2);
    h.get_all_by_label(&option_label).last().expect("substituted face option").click();
    h.run_steps(2);
    assert_eq!(h.state().look.unwrap().font, FieldFont::Thai(sarabun));

    // Reopening a fresh draft for the same field ("city") with Sarabun must NOT show the notice.
    let mut fresh = pdfcraft_ui_egui::prepare::FieldDraft::default();
    fresh.field = "city".into();
    fresh.look = Some(pdfcraft_engine::FieldLook {
        border: None,
        fill: None,
        width: 1.0,
        style: pdfcraft_engine::BorderStyle::Solid,
        text: [0.0; 3],
        font: FieldFont::Thai(sarabun),
    });
    fresh.face = Some(pdfcraft_engine::FieldFace::Thai(sarabun));
    *h.state_mut() = fresh;
    h.run_steps(2);
    let notice = format!("{} can't be embedded in a form. The field will use Sarabun.", sub.family);
    assert!(h.query_all_by_label_contains(&notice).next().is_none(), "substitution notice must not leak into a reopened dialog");
}
