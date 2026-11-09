//! WikiCraft document model: ProseMirror-compatible JSON documents and conversions
//! (Yjs/y-prosemirror, Markdown, HTML, plain text).

pub mod html;
pub mod markdown;
pub mod node;
pub mod text;
pub mod ydoc;

pub use node::{Mark, Node, marks, nodes};
