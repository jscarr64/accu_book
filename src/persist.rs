//! Plain-text `.accu` persistence for headless notebooks.
//!
//! On-disk format is line-oriented UTF-8 (not JSON). Transient session state
//! (results, job queue, chrome) is never written.
//!
//! Source bodies are length-prefixed (`SOURCE <byte_len>`) so cell text may
//! contain any UTF-8, including lines that look like delimiters.
//! Line endings may be `\n` or `\r\n`. A metadata pair that fits on one line
//! is `META key value`. A key with a space, or a value with a newline, is
//! length-prefixed as `METABYTES` so it round-trips.

use crate::{Cell, CellId, CellKind, IntakePolicy, Notebook};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Errors from reading or writing `.accu` files.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid .accu: {0}")]
    Format(String),
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
            write_meta(&mut out, k, v);
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
        let mut end = *pos;
        if *pos < bytes.len() && bytes[*pos] == b'\n' {
            *pos += 1;
        }
        // A CRLF terminator leaves the carriage return on the line.
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
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
            let policy = IntakePolicy::parse(rest)
                .ok_or_else(|| PersistError::Format(format!("unknown intake `{rest}`")))?;
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
            return Err(PersistError::Format(format!("expected CELL, got `{line}`")));
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
            if let Some(rest) = peek.strip_prefix("METABYTES ") {
                let (k, v) = read_metabytes(bytes, &mut pos, rest)?;
                metadata.insert(k, v);
            } else if let Some(rest) = peek.strip_prefix("META ") {
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
        let byte_len: usize = len_str
            .parse()
            .map_err(|_| PersistError::Format(format!("bad SOURCE length `{len_str}`")))?;
        let source = read_utf8(bytes, &mut pos, byte_len, "SOURCE")?;
        expect_newline(bytes, &mut pos, "SOURCE body")?;

        let end = take_line(bytes, &mut pos)?;
        if end.trim() != "ENDCELL" {
            return Err(PersistError::Format(format!(
                "expected ENDCELL, got `{end}`"
            )));
        }

        if nb.get_by_id(&id).is_some() {
            return Err(PersistError::Format(format!("duplicate cell id `{id}`")));
        }
        nb.append(Cell::from_parts(id, kind, source, metadata));
    }
}

fn fits_meta_line(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains([' ', '\n', '\r']) && !value.contains(['\n', '\r'])
}

fn write_meta(out: &mut String, key: &str, value: &str) {
    if fits_meta_line(key, value) {
        out.push_str("META ");
        out.push_str(key);
        out.push(' ');
        out.push_str(value);
        out.push('\n');
        return;
    }
    out.push_str("METABYTES ");
    out.push_str(&key.len().to_string());
    out.push(' ');
    out.push_str(&value.len().to_string());
    out.push('\n');
    out.push_str(key);
    out.push_str(value);
    out.push('\n');
}

fn read_metabytes(
    bytes: &[u8],
    pos: &mut usize,
    rest: &str,
) -> Result<(String, String), PersistError> {
    let mut parts = rest.split_whitespace();
    let key_len = parse_usize(parts.next(), "METABYTES key length")?;
    let value_len = parse_usize(parts.next(), "METABYTES value length")?;
    if parts.next().is_some() {
        return Err(PersistError::Format(format!(
            "bad METABYTES header `{rest}`"
        )));
    }
    let key = read_utf8(bytes, pos, key_len, "META key")?;
    let value = read_utf8(bytes, pos, value_len, "META value")?;
    expect_newline(bytes, pos, "METABYTES")?;
    Ok((key, value))
}

fn parse_usize(text: Option<&str>, what: &str) -> Result<usize, PersistError> {
    let text = text.ok_or_else(|| PersistError::Format(format!("missing {what}")))?;
    text.parse()
        .map_err(|_| PersistError::Format(format!("bad {what} `{text}`")))
}

fn read_utf8(
    bytes: &[u8],
    pos: &mut usize,
    len: usize,
    what: &str,
) -> Result<String, PersistError> {
    if pos.saturating_add(len) > bytes.len() {
        return Err(PersistError::Format(format!(
            "{what} length past end of file"
        )));
    }
    let text = std::str::from_utf8(&bytes[*pos..*pos + len])
        .map_err(|_| PersistError::Format(format!("{what} is not valid UTF-8")))?
        .to_string();
    *pos += len;
    Ok(text)
}

fn expect_newline(bytes: &[u8], pos: &mut usize, what: &str) -> Result<(), PersistError> {
    if *pos < bytes.len()
        && bytes[*pos] == b'\r'
        && *pos + 1 < bytes.len()
        && bytes[*pos + 1] == b'\n'
    {
        *pos += 2;
        return Ok(());
    }
    if *pos < bytes.len() && bytes[*pos] == b'\n' {
        *pos += 1;
        return Ok(());
    }
    Err(PersistError::Format(format!(
        "expected newline after {what}"
    )))
}

/// Write `nb` to `path` as UTF-8 `.accu`.
///
/// The new text goes to a sibling temp file, which is fsynced, and that file
/// is then renamed over `path`. A crash before the rename leaves the previous
/// notebook in place. `File::create` on `path` would empty it first.
pub fn save_notebook(path: &Path, nb: &Notebook) -> Result<(), PersistError> {
    let text = encode_notebook(nb);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = temp_beside(path)?;
    if let Err(err) = write_synced(&tmp, text.as_bytes()) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = replace_file(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(PersistError::Io(err));
    }
    Ok(())
}

fn temp_beside(path: &Path) -> Result<PathBuf, PersistError> {
    let file_name = path.file_name().ok_or_else(|| {
        PersistError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "notebook path has no file name",
        ))
    })?;
    let mut name = OsString::from(file_name);
    name.push(".");
    name.push(Uuid::new_v4().simple().to_string());
    name.push(".tmp");
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    Ok(match parent {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    })
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), PersistError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        // Windows will not rename over a file that is already there.
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(to)?;
            fs::rename(from, to)
        }
        Err(err) => Err(err),
    }
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
    fn replace_leaves_one_complete_file() {
        let mut first = Notebook::new();
        first.append(Cell::new_code("old".into()));
        let mut second = Notebook::new();
        second.append(Cell::new_code("new".into()));
        let dir = std::env::temp_dir().join(format!("accu_persist_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.accu");
        save_notebook(&path, &first).expect("first");
        let previous = fs::read(&path).expect("read previous");
        save_notebook(&path, &second).expect("second");
        let loaded = load_notebook(&path).expect("load");
        assert_eq!(loaded, second);
        assert_ne!(fs::read(&path).expect("read replaced"), previous);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![OsString::from("demo.accu")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_rejects_a_path_with_no_file_name() {
        let err = save_notebook(Path::new("/"), &Notebook::new()).unwrap_err();
        match err {
            PersistError::Io(err) => assert_eq!(err.kind(), io::ErrorKind::InvalidInput),
            PersistError::Format(_) => panic!("expected an i/o error"),
        }
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

    #[test]
    fn crlf_file_loads() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("x = 1".into()));
        let text = encode_notebook(&nb).replace('\n', "\r\n");
        let nb2 = decode_notebook(&text).expect("crlf");
        assert_eq!(nb, nb2);

        let id = nb2.get(0).unwrap().id();
        let src = "a\nb";
        let multiline = format!(
            "accu 1\r\nintake explicit\r\n\r\nCELL {id}\r\nKIND code\r\nSOURCE {}\r\n{src}\r\nENDCELL\r\n",
            src.len()
        );
        let nb3 = decode_notebook(&multiline).expect("crlf around a lf body");
        assert_eq!(nb3.get(0).unwrap().source(), src);
    }

    #[test]
    fn metadata_space_and_newline_round_trip() {
        let mut nb = Notebook::new();
        let cell = Cell::new_code("1".into());
        let id = cell.id();
        nb.append(cell);
        let mut meta = BTreeMap::new();
        meta.insert("title".into(), "hello world".into());
        meta.insert("my key".into(), "hello\nworld".into());
        nb.set_metadata(&id, meta).unwrap();
        let text = encode_notebook(&nb);
        assert!(text.contains("META title hello world\n"));
        assert!(text.contains("METABYTES "));
        let nb2 = decode_notebook(&text).expect("decode");
        assert_eq!(nb, nb2);
        assert_eq!(
            nb2.get(0)
                .unwrap()
                .metadata()
                .get("my key")
                .map(String::as_str),
            Some("hello\nworld")
        );
    }

    #[test]
    fn duplicate_cell_id_is_refused() {
        let id = "00000000-0000-0000-0000-000000000001";
        let text = format!(
            "accu 1\nintake explicit\n\nCELL {id}\nKIND code\nSOURCE 1\nx\nENDCELL\n\nCELL {id}\nKIND code\nSOURCE 1\ny\nENDCELL\n"
        );
        let err = decode_notebook(&text).unwrap_err();
        match err {
            PersistError::Format(message) => assert!(message.contains("duplicate cell id")),
            PersistError::Io(_) => panic!("expected a format error"),
        }
    }
}
