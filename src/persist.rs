//! Plain-text `.accu` persistence for headless notebooks.
//!
//! On-disk format is line-oriented UTF-8 (not JSON). Transient session state
//! (results, job queue, chrome) is never written.

use crate::{Cell, CellId, CellKind, Notebook};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use uuid::Uuid;

/// Errors from reading or writing `.accu` files.
#[derive(Debug)]
pub enum PersistError {
    Io(io::Error),
    Format(String),
}

impl fmt::Display for PersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PersistError::Io(e) => write!(f, "I/O error: {e}"),
            PersistError::Format(msg) => write!(f, "invalid .accu: {msg}"),
        }
    }
}

impl std::error::Error for PersistError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PersistError::Io(e) => Some(e),
            PersistError::Format(_) => None,
        }
    }
}

impl From<io::Error> for PersistError {
    fn from(value: io::Error) -> Self {
        PersistError::Io(value)
    }
}

const MAGIC: &str = "accu 1";

/// Encode a notebook to the plain-text `.accu` format.
pub fn encode_notebook(nb: &Notebook) -> String {
    let mut out = String::new();
    out.push_str(MAGIC);
    out.push('\n');
    out.push('\n');
    for cell in nb.iter() {
        out.push_str("CELL ");
        out.push_str(&cell.id().to_string());
        out.push('\n');
        out.push_str("KIND ");
        out.push_str(match cell.kind() {
            CellKind::Code => "code",
            CellKind::Markdown => "markdown",
        });
        out.push('\n');
        for (k, v) in cell.metadata() {
            out.push_str("META ");
            out.push_str(k);
            out.push(' ');
            out.push_str(v);
            out.push('\n');
        }
        out.push_str("SOURCE\n");
        out.push_str(cell.source());
        if !cell.source().is_empty() && !cell.source().ends_with('\n') {
            out.push('\n');
        }
        out.push_str("ENDSOURCE\n");
        out.push_str("ENDCELL\n");
        out.push('\n');
    }
    out
}

/// Decode a notebook from plain-text `.accu` text.
pub fn decode_notebook(text: &str) -> Result<Notebook, PersistError> {
    let mut lines = text.lines().peekable();
    let magic = lines
        .next()
        .ok_or_else(|| PersistError::Format("empty file".into()))?;
    if magic.trim() != MAGIC {
        return Err(PersistError::Format(format!(
            "expected `{MAGIC}` header, got `{magic}`"
        )));
    }

    let mut nb = Notebook::new();
    while let Some(line) = lines.next() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("CELL ") {
            return Err(PersistError::Format(format!(
                "expected CELL, got `{line}`"
            )));
        }
        let id_str = line[5..].trim();
        let uuid = Uuid::parse_str(id_str)
            .map_err(|_| PersistError::Format(format!("bad cell id `{id_str}`")))?;
        let id = CellId::from(uuid);

        let kind_line = lines
            .next()
            .ok_or_else(|| PersistError::Format("missing KIND".into()))?;
        if !kind_line.starts_with("KIND ") {
            return Err(PersistError::Format(format!(
                "expected KIND, got `{kind_line}`"
            )));
        }
        let kind = match kind_line[5..].trim() {
            "code" => CellKind::Code,
            "markdown" => CellKind::Markdown,
            other => {
                return Err(PersistError::Format(format!("unknown kind `{other}`")));
            }
        };

        let mut metadata = BTreeMap::new();
        loop {
            let peek = lines.peek().copied().unwrap_or("");
            if peek.starts_with("META ") {
                let meta_line = lines.next().unwrap_or("");
                let rest = &meta_line[5..];
                let (k, v) = rest
                    .split_once(' ')
                    .ok_or_else(|| PersistError::Format("META needs key value".into()))?;
                metadata.insert(k.to_string(), v.to_string());
            } else {
                break;
            }
        }

        let src_hdr = lines
            .next()
            .ok_or_else(|| PersistError::Format("missing SOURCE".into()))?;
        if src_hdr.trim() != "SOURCE" {
            return Err(PersistError::Format(format!(
                "expected SOURCE, got `{src_hdr}`"
            )));
        }
        let mut source = String::new();
        loop {
            let Some(src_line) = lines.next() else {
                return Err(PersistError::Format("unterminated SOURCE".into()));
            };
            if src_line == "ENDSOURCE" {
                break;
            }
            source.push_str(src_line);
            source.push('\n');
        }
        if source.ends_with('\n') {
            source.pop();
        }

        let end = lines
            .next()
            .ok_or_else(|| PersistError::Format("missing ENDCELL".into()))?;
        if end.trim() != "ENDCELL" {
            return Err(PersistError::Format(format!(
                "expected ENDCELL, got `{end}`"
            )));
        }

        nb.append(Cell::from_parts(id, kind, source, metadata));
    }
    Ok(nb)
}

/// Write `nb` to `path` as UTF-8 `.accu`.
pub fn save_notebook(path: &Path, nb: &Notebook) -> Result<(), PersistError> {
    let text = encode_notebook(nb);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut f = fs::File::create(path)?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

/// Read a notebook from `path`.
pub fn load_notebook(path: &Path) -> Result<Notebook, PersistError> {
    let text = fs::read_to_string(path)?;
    decode_notebook(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_memory() {
        let mut nb = Notebook::new();
        let c = Cell::new_code("1+1".into());
        let id = c.id();
        nb.append(c);
        let mut meta = BTreeMap::new();
        meta.insert("title".into(), "intro".into());
        nb.set_metadata(&id, meta).unwrap();
        nb.append(Cell::new_markdown("# Hi".into()));

        let text = encode_notebook(&nb);
        let nb2 = decode_notebook(&text).expect("decode");
        assert_eq!(nb, nb2);
    }

    #[test]
    fn round_trip_file() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("x = 1".into()));
        let dir = std::env::temp_dir().join(format!("accu_persist_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.accu");
        save_notebook(&path, &nb).expect("save");
        let nb2 = load_notebook(&path).expect("load");
        assert_eq!(nb, nb2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_notebook() {
        let nb = Notebook::new();
        let text = encode_notebook(&nb);
        assert!(text.starts_with("accu 1\n"));
        let nb2 = decode_notebook(&text).unwrap();
        assert!(nb2.is_empty());
    }

    #[test]
    fn rejects_bad_magic() {
        let err = decode_notebook("not-accu\n").unwrap_err();
        match err {
            PersistError::Format(m) => assert!(m.contains("accu 1")),
            PersistError::Io(_) => panic!("expected format error"),
        }
    }

    #[test]
    fn multiline_source() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a\nb\nc".into()));
        let nb2 = decode_notebook(&encode_notebook(&nb)).unwrap();
        assert_eq!(nb.get(0).unwrap().source(), "a\nb\nc");
        assert_eq!(nb, nb2);
    }
}
