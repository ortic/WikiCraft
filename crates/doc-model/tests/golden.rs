//! Golden tests for Markdown import (`tests/golden/*.md`): the document it produces, how it
//! exports back to Markdown and how it renders as HTML. Snapshot diffs are reviewed and accepted
//! with `INSTA_UPDATE=always cargo test -p wikicraft-doc-model --test golden`.

use wikicraft_doc_model::{html, markdown, ydoc};

#[test]
fn markdown_import() {
    insta::glob!("golden/*.md", |path| {
        let src = std::fs::read_to_string(path).unwrap();
        let doc = markdown::markdown_to_pm(&src).doc;
        // what the editor works on: the Yjs encoding gives the same document back
        assert_eq!(ydoc::update_to_pm(&ydoc::pm_to_update(&doc)).unwrap(), doc);
        let exported = markdown::pm_to_markdown(&doc);
        // exporting what was imported is stable: a second round trip changes nothing
        let again = markdown::pm_to_markdown(&markdown::markdown_to_pm(&exported).doc);
        assert_eq!(again, exported, "Markdown export isn't stable");
        insta::assert_snapshot!(format!(
            "## prosemirror\n{}\n\n## markdown\n{}\n\n## html\n{}\n",
            serde_json::to_string_pretty(&doc).unwrap(),
            exported.trim_end(),
            html::render(&doc, &html::HtmlOptions { heading_ids: true }).replace("><", ">\n<"),
        ));
    });
}
