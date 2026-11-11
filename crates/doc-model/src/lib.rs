//! WikiCraft document model: ProseMirror-compatible JSON documents and conversions
//! (Yjs/y-prosemirror, Markdown, HTML, plain text).

pub mod blocks;
pub mod chunk;
pub mod html;
pub mod icons;
pub mod markdown;
pub mod node;
pub mod schema;
pub mod template;
pub mod text;
pub mod toc;
pub mod ydoc;

pub use node::{Mark, Node, marks, nodes};
