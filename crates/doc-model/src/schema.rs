//! Versions of the stored document format, and how older documents are brought up to date.
//!
//! The version is stored per page (`pages.schema_version`). When the format changes, bump
//! [`VERSION`], add a step to [`upgrade`] and [`upgrade_ydoc`] (both must do the same thing:
//! one on ProseMirror JSON, one on the live Yjs state, as CRDT operations), and add a database
//! migration that sets the column's default to the new version. The server upgrades old pages
//! in the background and when they are opened.
//!
//! | Version | Change |
//! |---|---|
//! | 1 | ProseMirror / Tiptap JSON as of the first release |
//! | 2 | every block carries a stable `blockId` ([`crate::blocks`]) |

use crate::blocks;
use crate::node::Node;

/// The format the current code writes.
pub const VERSION: i32 = 2;

/// Upgrades a document stored at version `from`; returns whether anything changed.
pub fn upgrade(doc: &mut Node, from: i32) -> bool {
    let mut changed = false;
    if from < 2 {
        changed |= blocks::ensure_ids(doc) > 0;
    }
    changed
}

/// [`upgrade`] for a live Yjs document.
pub fn upgrade_ydoc(doc: &yrs::Doc, from: i32) -> bool {
    let mut changed = false;
    if from < 2 {
        changed |= blocks::ensure_ydoc_ids(doc);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{nodes, ydoc};

    #[test]
    fn json_and_yjs_upgrades_agree() {
        let v1 = Node::doc(vec![
            Node::heading(1, vec![Node::text("T")]),
            Node::new(nodes::BULLET_LIST).with_content(vec![Node::new(nodes::LIST_ITEM).with_content(vec![Node::paragraph(vec![Node::text("a")])])]),
        ]);
        let mut json = v1.clone();
        assert!(upgrade(&mut json, 1));
        assert!(!upgrade(&mut json, VERSION), "nothing to do at the current version");
        let y = ydoc::new_doc();
        ydoc::write_pm(&y, &v1);
        assert!(upgrade_ydoc(&y, 1));
        let mut from_y = ydoc::doc_to_pm(&y);
        // same shape; ids are random, so compare after giving both the same ones
        crate::blocks::strip_ids(&mut from_y);
        let mut stripped = json.clone();
        crate::blocks::strip_ids(&mut stripped);
        assert_eq!(from_y, stripped);
        assert_eq!(crate::blocks::ensure_ids(&mut json), 0);
    }
}
