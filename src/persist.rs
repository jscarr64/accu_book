//! Plain-text `.accu` persistence for headless notebooks.
//!
//! On-disk format is line-oriented UTF-8 (not JSON). Transient session state
//! (results, job queue, chrome) is never written.
//!
//! Source bodies are length-prefixed (`SOURCE <byte_len>`) so cell text may
//! contain any UTF-8, including lines that look like delimiters.

use crate::{Cell, CellId, CellKind, IntakePolicy, Notebook};
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
    out.push_str("intake ");
    out.push_str(nb.intake_policy().as_str());
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
            // Keys must not contain spaces; values must be single-line (documented).
            out.push_str("META ");
            out.push_str(k);
            out.push(' ');
            out.push_str(v);
            out.push('\n');
        }
        let src = cell.source();
        out.push_str(&format!("SOURCE {}\n", src.len()));
        out.push_str(src);
        out.push('\n');
        out.push_str("ENDCELL\n");
        out.push('\n');
    }
    out
}

/// Decode a notebook from plain-text `.accu` text.
pub fn decode_notebook(text: &str) -> Result<Notebook, PersistError> {
    let bytes = text.as_bytes();
    let mut pos = 0usize;

    fn take_line<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<&'a str, PersistError> {
        if *pos >= bytes.len() {
            return Err(PersistError::Format("unexpected end of file".into()));
        }
        let start = *pos;
        while *pos < bytes.len() && bytes[*pos] != b'\n' {
            *pos += 1;
        }
        let end = *pos;
        if *pos < bytes.len() && bytes[*pos] == b'\n' {
            *pos += 1;
        }
        std::str::from_utf8(&bytes[start..end])
            .map_err(|_| PersistError::Format("non-UTF-8 line".into()))
    }

    let magic = take_line(bytes, &mut pos)?;
    if magic.trim() != MAGIC {
        return Err(PersistError::Format(format!(
            "expected `{MAGIC}` header, got `{magic}`"
        )));
    }

    let mut nb = Notebook::new();

    // Optional file-level intake (default Explicit if omitted — S6 files).
    if pos < bytes.len() {
        let save = pos;
        let line = take_line(bytes, &mut pos)?;
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("intake ") {
            let policy = IntakePolicy::parse(rest).ok_or_else(|| {
                PersistError::Format(format!("unknown intake `{rest}`"))
            })?;
            nb.set_intake_policy(policy);
        } else {
            pos = save;
        }
    }

    loop {
        // skip blank lines
        loop {
            if pos >= bytes.len() {
                return Ok(nb);
            }
            let save = pos;
            let line = take_line(bytes, &mut pos)?;
            if line.trim().is_empty() {
                continue;
            }
            pos = save;
            break;
        }

        let line = take_line(bytes, &mut pos)?;
        let line = line.trim_end();
        let Some(id_str) = line.strip_prefix("CELL ") else {
            return Err(PersistError::Format(format!(
                "expected CELL, got `{line}`"
            )));
        };
        let id_str = id_str.trim();
        let uuid = Uuid::parse_str(id_str)
            .map_err(|_| PersistError::Format(format!("bad cell id `{id_str}`")))?;
        let id = CellId::from(uuid);

        let kind_line = take_line(bytes, &mut pos)?;
        let Some(kind_raw) = kind_line.strip_prefix("KIND ") else {
            return Err(PersistError::Format(format!(
                "expected KIND, got `{kind_line}`"
            )));
        };
        let kind = match kind_raw.trim() {
            "code" => CellKind::Code,
            "markdown" => CellKind::Markdown,
            other => {
                return Err(PersistError::Format(format!("unknown kind `{other}`")));
            }
        };

        let mut metadata = BTreeMap::new();
        loop {
            if pos >= bytes.len() {
                return Err(PersistError::Format("missing SOURCE".into()));
            }
            let save = pos;
            let peek = take_line(bytes, &mut pos)?;
            if let Some(rest) = peek.strip_prefix("META ") {
                let (k, v) = rest
                    .split_once(' ')
                    .ok_or_else(|| PersistError::Format("META needs key value".into()))?;
                metadata.insert(k.to_string(), v.to_string());
            } else {
                pos = save;
                break;
            }
        }

        let src_hdr = take_line(bytes, &mut pos)?;
        let src_hdr = src_hdr.trim_end();
        let Some(len_str) = src_hdr.strip_prefix("SOURCE ") else {
            return Err(PersistError::Format(format!(
                "expected SOURCE <len>, got `{src_hdr}`"
            )));
        };
        let len_str = len_str.trim();
        let byte_len: usize = len_str.parse().map_err(|_| {
            PersistError::Format(format!("bad SOURCE length `{len_str}`"))
        })?;
        if pos + byte_len > bytes.len() {
            return Err(PersistError::Format("SOURCE length past end of file".into()));
        }
        let source = std::str::from_utf8(&bytes[pos..pos + byte_len])
            .map_err(|_| PersistError::Format("SOURCE is not valid UTF-8".into()))?
            .to_string();
        pos += byte_len;
        // encoder always writes a trailing newline after the source bytes
        if pos < bytes.len() && bytes[pos] == b'\n' {
            pos += 1;
        } else {
            return Err(PersistError::Format(
                "expected newline after SOURCE body".into(),
            ));
        }

        let end = take_line(bytes, &mut pos)?;
        if end.trim() != "ENDCELL" {
            return Err(PersistError::Format(format!(
                "expected ENDCELL, got `{end}`"
            )));
        }

        nb.append(Cell::from_parts(id, kind, source, metadata));
    }
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

    #[test]
    fn intake_policy_round_trip() {
        for policy in [IntakePolicy::Explicit, IntakePolicy::Inference] {
            let mut nb = Notebook::new();
            nb.set_intake_policy(policy);
            nb.append(Cell::new_code("1".into()));
            let nb2 = decode_notebook(&encode_notebook(&nb)).unwrap();
            assert_eq!(nb2.intake_policy(), policy);
            assert_eq!(nb, nb2);
        }
    }

    #[test]
    fn missing_intake_line_defaults_explicit() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("x".into()));
        let full = encode_notebook(&nb);
        // Drop the `intake …` line (S6-shaped files).
        let mut out = String::from("accu 1\n\n");
        let mut skip_intake = true;
        for line in full.lines().skip(1) {
            if skip_intake && line.starts_with("intake ") {
                skip_intake = false;
                continue;
            }
            skip_intake = false;
            out.push_str(line);
            out.push('\n');
        }
        let nb2 = decode_notebook(&out).unwrap();
        assert_eq!(nb2.intake_policy(), IntakePolicy::Explicit);
        assert_eq!(nb2.len(), 1);
    }

    #[test]
    fn source_containing_endsource_line() {
        let mut nb = Notebook::new();
        let body = "before\nENDSOURCE\nafter\nENDCELL\nok";
        nb.append(Cell::new_markdown(body.into()));
        let text = encode_notebook(&nb);
        assert!(text.contains("SOURCE "));
        // Body may contain ENDSOURCE; length prefix is what makes decode safe.
        assert!(text.contains("ENDSOURCE"));
        let nb2 = decode_notebook(&text).expect("decode with delimiter collision payload");
        assert_eq!(nb2.get(0).unwrap().source(), body);
        assert_eq!(nb, nb2);
    }
}
