//! Document titles and file names in Thai, Japanese and Chinese reach the tab and window
//! title unchanged, and an unusable title falls back to the file name. Fork invariant.

use pdfcraft_engine::{Edit, Session};
use std::sync::Arc;

/// Opens a blank page under `file_name`, sets `title` with "show the title" on, and reopens
/// the saved bytes so the name has been through a real write and read.
fn display_name_after_reopen(file_name: &str, title: &str) -> String {
    let mut session = Session::new();
    let bytes = session.create_blank(612.0, 792.0, 1).expect("blank page");
    let id = session.open(file_name, None, bytes, None).expect("open blank page");
    let mut view = session.get(id).expect("document").initial_view();
    view.display_title = true;
    session
        .apply(
            id,
            Edit::Batch {
                label: "Set document title".into(),
                edits: vec![Edit::SetInfo { key: "Title".into(), value: title.into() }, Edit::SetInitialView(Box::new(view))],
            },
        )
        .expect("set title");
    let saved = Arc::new(session.get(id).expect("document").bytes.to_vec());
    let reopened = session.open(file_name, None, saved, None).expect("reopen");
    session.get(reopened).expect("reopened document").display_name()
}

#[test]
fn thai_japanese_and_chinese_titles_survive_a_save_and_reopen() {
    for title in ["รายงานประจำปี ๒๕๖๙", "日本語のタイトル", "简体中文标题", "繁體中文標題"] {
        assert_eq!(display_name_after_reopen("file.pdf", title), title);
    }
}

#[test]
fn unusable_titles_fall_back_to_a_thai_or_cjk_file_name() {
    for file_name in ["รายงาน.pdf", "資料.pdf"] {
        for title in ["   ", "untitled", "about:blank"] {
            assert_eq!(display_name_after_reopen(file_name, title), file_name, "title {title:?}");
        }
    }
}
