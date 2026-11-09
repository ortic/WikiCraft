//! Access to export files, whether the export is a `.zip` or an extracted directory.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

/// Hard limits protecting against zip bombs and absurd inputs.
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024 * 1024; // 64 GiB uncompressed per entry

#[derive(Debug)]
pub enum Source {
    Zip { path: PathBuf, names: Vec<String>, prefix: String },
    Dir { root: PathBuf, names: Vec<String> },
}

impl Source {
    pub fn open(path: &Path) -> io::Result<Self> {
        if path.is_dir() {
            let mut names = Vec::new();
            collect_files(path, path, &mut names)?;
            names.sort();
            return Ok(Source::Dir { root: path.to_path_buf(), names });
        }
        let file = File::open(path)?;
        let zip = zip::ZipArchive::new(BufReader::new(file)).map_err(io::Error::other)?;
        let mut names: Vec<String> = Vec::with_capacity(zip.len());
        for name in zip.file_names().flatten() {
            // zip-slip protection: we never write entries to disk by name, but reject odd paths anyway
            if name.contains("..") || name.starts_with('/') || name.contains('\\') {
                continue;
            }
            names.push(name.to_string());
        }
        names.sort();
        // Some tools zip the export inside a top-level folder: detect the folder containing the descriptor.
        let prefix = names
            .iter()
            .filter(|n| n.ends_with("exportDescriptor.properties"))
            .min_by_key(|n| n.len())
            .map(|n| n.trim_end_matches("exportDescriptor.properties").to_string())
            .unwrap_or_default();
        Ok(Source::Zip { path: path.to_path_buf(), names, prefix })
    }

    /// Entry names relative to the export root.
    pub fn names(&self) -> Vec<String> {
        match self {
            Source::Zip { names, prefix, .. } => {
                names.iter().filter_map(|n| n.strip_prefix(prefix.as_str())).filter(|n| !n.is_empty() && !n.ends_with('/')).map(String::from).collect()
            }
            Source::Dir { names, .. } => names.clone(),
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.names().iter().any(|n| n == name)
    }

    /// Opens an entry for streaming reads; `.gz` entries are transparently decompressed.
    pub fn open_entry(&self, name: &str) -> io::Result<Box<dyn Read + Send>> {
        let raw: Box<dyn Read + Send> = match self {
            Source::Dir { root, .. } => {
                let p = root.join(name);
                if !p.starts_with(root) {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "path escapes export root"));
                }
                Box::new(BufReader::new(File::open(p)?))
            }
            Source::Zip { path, prefix, .. } => {
                let file = File::open(path)?;
                let zip = zip::ZipArchive::new(BufReader::new(file)).map_err(io::Error::other)?;
                let full = format!("{prefix}{name}");
                let idx = zip.index_for_name(&full).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, full.clone()))?;
                // read lazily via an owned archive wrapper
                Box::new(ZipEntryReader::new(zip, idx)?)
            }
        };
        if name.ends_with(".gz") { Ok(Box::new(BufReader::new(GzDecoder::new(raw)))) } else { Ok(raw) }
    }

    pub fn read_to_string(&self, name: &str) -> io::Result<String> {
        let mut s = String::new();
        self.open_entry(name)?.read_to_string(&mut s)?;
        Ok(s)
    }

    /// Reads a whole binary entry (attachments).
    pub fn read_bytes(&self, name: &str) -> io::Result<Vec<u8>> {
        let mut v = Vec::new();
        self.open_entry(name)?.take(MAX_ENTRY_BYTES).read_to_end(&mut v)?;
        Ok(v)
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_files(root, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Streams one zip entry while owning the archive: a worker thread decompresses into an OS
/// pipe (zip entry readers borrow the archive, so they can't be returned directly). Errors in
/// the worker are reported when the stream ends instead of silently truncating it.
struct ZipEntryReader {
    pipe: io::PipeReader,
    worker: Option<std::thread::JoinHandle<io::Result<u64>>>,
}

impl ZipEntryReader {
    fn new(mut archive: zip::ZipArchive<BufReader<File>>, idx: usize) -> io::Result<Self> {
        let size = archive.by_index(idx).map_err(io::Error::other)?.size();
        if size > MAX_ENTRY_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "zip entry too large"));
        }
        let (reader, mut writer) = io::pipe()?;
        let worker = std::thread::spawn(move || -> io::Result<u64> {
            let mut entry = archive.by_index(idx).map_err(io::Error::other)?;
            let n = io::copy(&mut (&mut entry).take(MAX_ENTRY_BYTES), &mut writer);
            match n {
                // reader dropped early (caller stopped reading) — not an error
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
                other => other,
            }
        });
        Ok(Self { pipe: reader, worker: Some(worker) })
    }
}

impl Read for ZipEntryReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.pipe.read(out)?;
        if n == 0
            && !out.is_empty()
            && let Some(w) = self.worker.take()
        {
            match w.join() {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(io::Error::other("zip worker panicked")),
            }
        }
        Ok(n)
    }
}

/// Parses a Java `.properties` file (the export descriptor).
pub fn parse_properties(s: &str) -> std::collections::BTreeMap<String, String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .filter_map(|l| {
            let i = l.find(['=', ':'])?;
            Some((l[..i].trim().to_string(), l[i + 1..].trim().replace("\\:", ":").replace("\\=", "=")))
        })
        .collect()
}
