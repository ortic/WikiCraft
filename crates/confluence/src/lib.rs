//! Confluence export reader and storage-format converter for WikiCraft.
//!
//! ```no_run
//! let export = wikicraft_confluence::open(std::path::Path::new("Confluence-space-export.zip")).unwrap();
//! let planner = wikicraft_confluence::Planner::new(&export, Default::default());
//! let plan = planner.plan();
//! println!("{} pages", plan.report.counts.get("pages").unwrap_or(&0));
//! ```

pub mod bodies;
pub mod csv_export;
pub mod dom;
pub mod entities;
pub mod model;
pub mod plan;
pub mod source;
pub mod storage;
pub mod urls;
pub mod wiki;
pub mod xml_export;

use std::path::Path;

pub use model::{Export, ExportFormat};
pub use plan::{ImportPlan, PlanOptions, Planner, Report};
pub use source::Source;
pub use storage::Stats;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a Confluence export: {0}")]
    Format(String),
    #[error("XML error: {0}")]
    Xml(String),
    #[error("CSV error: {0}")]
    Csv(String),
}

/// Opens a Confluence space/site export (zip file or extracted directory) and reads it fully.
pub fn open(path: &Path) -> Result<Export, Error> {
    let source = Source::open(path)?;
    read(&source)
}

/// Reads an export from an opened [`Source`].
pub fn read(source: &Source) -> Result<Export, Error> {
    let names = source.names();
    let mut export = Export::default();
    let descriptor = |n: &&String| n.rsplit('/').next().is_some_and(|b| b == "exportDescriptor.properties" || b == "exportDescriptor.properties.gz");
    if let Some(desc) = names.iter().find(descriptor) {
        export.descriptor = source::parse_properties(&source.read_to_string(desc)?);
    }
    let is_csv = export.descriptor.get("exportFormat").is_some_and(|f| f.eq_ignore_ascii_case("csv"))
        || names.iter().any(|n| n.ends_with("content.csv.gz") || n.ends_with("content.csv"));
    if is_csv {
        export.format = Some(ExportFormat::Csv);
        csv_export::read(source, &mut export)?;
    } else if let Some(entities) = names.iter().find(|n| n.rsplit('/').next() == Some("entities.xml")) {
        export.format = Some(ExportFormat::Xml);
        let input = source.open_entry(entities)?;
        xml_export::read(input, &mut export)?;
        xml_export::index_attachments(&names, &mut export);
    } else if names.iter().any(|n| n.ends_with(".html")) {
        return Err(Error::Format("this looks like a Confluence HTML export; use the HTML importer".into()));
    } else {
        return Err(Error::Format("neither entities.xml nor content.csv found".into()));
    }
    if let Some(e) = export.bodies.take_error() {
        return Err(Error::Io(std::io::Error::new(e.kind(), format!("storing page bodies in a temporary file: {e}"))));
    }
    if export.spaces.is_empty() {
        export.read_warnings.push("export contains no space".into());
    }
    Ok(export)
}
