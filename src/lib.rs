mod history;
mod dag;
mod jobs;
mod engine;
mod session;
mod persist;

pub use history::History;
pub use dag::{DepGraph, DepError};
pub use jobs::{Job, JobError, JobId, JobQueue, JobStatus};
pub use engine::{EchoEngine, EngineBridge, EngineError, EngineOp, EngineResult, NullEngine, eval_cell_with};
pub use session::{CellChrome, ResultStore, Session, SessionError, StoredOutput};
pub use persist::{decode_notebook, encode_notebook, load_notebook, save_notebook, PersistError};

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
pub struct CellId(Uuid);

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for CellId {
    fn from(uuid: Uuid) -> Self {
        CellId(uuid)
    }
}

impl CellId {
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}


/// How undeclared cell dependencies are handled at enqueue time.
///
/// Does **not** change the DAG engine — only intake. Explicit (default) requires
/// `Session::set_dependencies` before enqueue. Inference allows enqueue without
/// a prior declaration; this crate does **not** invent dependency edges — a host
/// that can analyze source may call `set_dependencies` itself.
///
/// Stored on the [`Notebook`] and written into `.accu` so reopen keeps the same rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntakePolicy {
    /// Refuse enqueue until dependencies were explicitly set (empty set is fine).
    #[default]
    Explicit,
    /// Allow enqueue without a prior `set_dependencies` call.
    Inference,
}

impl IntakePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            IntakePolicy::Explicit => "explicit",
            IntakePolicy::Inference => "inference",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "explicit" => Some(IntakePolicy::Explicit),
            "inference" => Some(IntakePolicy::Inference),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellKind {
    Code,
    Markdown,
}

impl CellKind {
    fn tag(self) -> &'static str {
        match self {
            CellKind::Code => "code",
            CellKind::Markdown => "markdown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    id: CellId,
    kind: CellKind,
    source: String,
    metadata: BTreeMap<String, String>,
}

impl Cell {
    pub fn new_code(source: String) -> Self {
        Cell {
            id: CellId(Uuid::new_v4()),
            kind: CellKind::Code,
            source,
            metadata: BTreeMap::new(),
        }
    }

    pub fn new_markdown(source: String) -> Self {
        Cell {
            id: CellId(Uuid::new_v4()),
            kind: CellKind::Markdown,
            source,
            metadata: BTreeMap::new(),
        }
    }

    /// Reconstruct a cell with a known id (used by `.accu` load).
    pub fn from_parts(
        id: CellId,
        kind: CellKind,
        source: String,
        metadata: BTreeMap<String, String>,
    ) -> Self {
        Cell {
            id,
            kind,
            source,
            metadata,
        }
    }

    pub fn id(&self) -> CellId {
        self.id
    }

    pub fn kind(&self) -> CellKind {
        self.kind
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notebook {
    cells: Vec<Cell>,
    #[serde(default)]
    intake: IntakePolicy,
}

impl Default for Notebook {
    fn default() -> Self {
        Self::new()
    }
}

impl Notebook {
    pub fn new() -> Self {
        Notebook {
            cells: Vec::new(),
            intake: IntakePolicy::Explicit,
        }
    }

    pub fn intake_policy(&self) -> IntakePolicy {
        self.intake
    }

    pub fn set_intake_policy(&mut self, policy: IntakePolicy) {
        self.intake = policy;
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn iter(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter()
    }

    pub fn cell_ids(&self) -> Vec<CellId> {
        self.cells.iter().map(|c| c.id).collect()
    }

    pub fn get(&self, index: usize) -> Option<&Cell> {
        self.cells.get(index)
    }

    pub fn get_by_id(&self, id: &CellId) -> Option<&Cell> {
        self.cells.iter().find(|cell| &cell.id == id)
    }

    pub fn append(&mut self, cell: Cell) {
        self.cells.push(cell);
    }

    pub fn insert(&mut self, index: usize, cell: Cell) -> Result<(), NotebookError> {
        if index > self.cells.len() {
            Err(NotebookError::IndexOutOfBounds)
        } else {
            self.cells.insert(index, cell);
            Ok(())
        }
    }

    pub fn remove(&mut self, id: &CellId) -> Result<Cell, NotebookError> {
        if let Some(index) = self.cells.iter().position(|cell| &cell.id == id) {
            Ok(self.cells.remove(index))
        } else {
            Err(NotebookError::UnknownCellId)
        }
    }

    pub fn move_cell(&mut self, from: usize, to: usize) -> Result<(), NotebookError> {
        if from >= self.cells.len() || to >= self.cells.len() {
            Err(NotebookError::IndexOutOfBounds)
        } else {
            let cell = self.cells.remove(from);
            self.cells.insert(to, cell);
            Ok(())
        }
    }

    pub fn set_source(&mut self, id: &CellId, source: String) -> Result<(), NotebookError> {
        if let Some(cell) = self.cells.iter_mut().find(|cell| &cell.id == id) {
            cell.source = source;
            Ok(())
        } else {
            Err(NotebookError::UnknownCellId)
        }
    }

    pub fn set_kind(&mut self, id: &CellId, kind: CellKind) -> Result<(), NotebookError> {
        if let Some(cell) = self.cells.iter_mut().find(|cell| &cell.id == id) {
            cell.kind = kind;
            Ok(())
        } else {
            Err(NotebookError::UnknownCellId)
        }
    }

    pub fn set_metadata(
        &mut self,
        id: &CellId,
        metadata: BTreeMap<String, String>,
    ) -> Result<(), NotebookError> {
        if let Some(cell) = self.cells.iter_mut().find(|cell| &cell.id == id) {
            cell.metadata = metadata;
            Ok(())
        } else {
            Err(NotebookError::UnknownCellId)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NotebookError {
    #[error("Index out of bounds")]
    IndexOutOfBounds,
    #[error("Unknown cell ID")]
    UnknownCellId,
}

/// Canonical cell content bytes for hashing (excludes id):
/// `kind_tag \0 source \0` then each metadata entry in `BTreeMap` order as `key \0 value \0`.
/// `kind_tag` is `code` or `markdown`.
fn update_cell_content(hasher: &mut Hasher, cell: &Cell) {
    hasher.update(cell.kind.tag().as_bytes());
    hasher.update(b"\0");
    hasher.update(cell.source.as_bytes());
    hasher.update(b"\0");
    for (k, v) in &cell.metadata {
        hasher.update(k.as_bytes());
        hasher.update(b"\0");
        hasher.update(v.as_bytes());
        hasher.update(b"\0");
    }
}

/// Blake3 hex of cell kind + source + metadata (see `update_cell_content`).
pub fn cell_content_hash(cell: &Cell) -> String {
    let mut hasher = Hasher::new();
    update_cell_content(&mut hasher, cell);
    hasher.finalize().to_hex().to_string()
}

/// Blake3 hex over each cell in order: `uuid_16 \0` + same bytes as [`cell_content_hash`].
pub fn notebook_content_hash(nb: &Notebook) -> String {
    let mut hasher = Hasher::new();
    for cell in &nb.cells {
        hasher.update(cell.id.0.as_bytes());
        hasher.update(b"\0");
        update_cell_content(&mut hasher, cell);
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let nb = Notebook::new();
        assert_eq!(nb.len(), 0);
        assert!(nb.cells().is_empty());
        assert!(nb.cell_ids().is_empty());
    }

    #[test]
    fn append_two_public_read() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        nb.append(Cell::new_markdown("2".to_string()));
        assert_eq!(nb.len(), 2);
        assert_eq!(nb.cells()[0].kind(), CellKind::Code);
        assert_eq!(nb.cells()[0].source(), "1");
        assert_eq!(nb.cells()[1].kind(), CellKind::Markdown);
        assert_eq!(nb.cell_ids().len(), 2);
        assert_eq!(nb.iter().count(), 2);
        assert!(nb.cells()[0].metadata().is_empty());
    }

    #[test]
    fn insert() {
        let mut nb = Notebook::new();
        nb.insert(0, Cell::new_code("1".to_string())).unwrap();
        nb.insert(1, Cell::new_markdown("2".to_string())).unwrap();
        assert_eq!(nb.len(), 2);
    }

    #[test]
    fn remove() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        nb.append(Cell::new_markdown("2".to_string()));
        nb.remove(&CellId::from(Uuid::new_v4())).unwrap_err();
        let id0 = nb.cells()[0].id();
        nb.remove(&id0).unwrap();
        assert_eq!(nb.len(), 1);
    }

    #[test]
    fn move_cell() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        nb.append(Cell::new_markdown("2".to_string()));
        nb.move_cell(0, 1).unwrap();
        assert_eq!(nb.cells()[0].kind(), CellKind::Markdown);
        assert_eq!(nb.cells()[1].kind(), CellKind::Code);
    }

    #[test]
    fn move_cell_three_from_lt_to() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".to_string()));
        nb.append(Cell::new_code("b".to_string()));
        nb.append(Cell::new_code("c".to_string()));
        nb.move_cell(0, 2).unwrap();
        assert_eq!(nb.cells()[0].source(), "b");
        assert_eq!(nb.cells()[1].source(), "c");
        assert_eq!(nb.cells()[2].source(), "a");
    }

    #[test]
    fn set_source_changes_hash() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        let old_hash = notebook_content_hash(&nb);
        let id = nb.cells()[0].id();
        nb.set_source(&id, "2".to_string()).unwrap();
        let new_hash = notebook_content_hash(&nb);
        assert_ne!(old_hash, new_hash);
    }

    #[test]
    fn cell_hash_deterministic_same_content() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("x".to_string()));
        let id = nb.cells()[0].id();
        let mut meta = BTreeMap::new();
        meta.insert("k".into(), "v".into());
        nb.set_metadata(&id, meta).unwrap();
        let h1 = cell_content_hash(&nb.cells()[0]);
        let h2 = cell_content_hash(&nb.cells()[0]);
        assert_eq!(h1, h2);
        // rebuild identical content via JSON round-trip
        let json = serde_json::to_string(&nb).unwrap();
        let nb2: Notebook = serde_json::from_str(&json).unwrap();
        assert_eq!(h1, cell_content_hash(&nb2.cells()[0]));
        assert_eq!(notebook_content_hash(&nb), notebook_content_hash(&nb2));
    }

    #[test]
    fn kind_change_flips_hashes() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("same".to_string()));
        let id = nb.cells()[0].id();
        let cell_before = cell_content_hash(&nb.cells()[0]);
        let nb_before = notebook_content_hash(&nb);
        nb.set_kind(&id, CellKind::Markdown).unwrap();
        assert_ne!(cell_before, cell_content_hash(&nb.cells()[0]));
        assert_ne!(nb_before, notebook_content_hash(&nb));
    }

    #[test]
    fn metadata_change_flips_hashes() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("same".to_string()));
        let id = nb.cells()[0].id();
        let cell_before = cell_content_hash(&nb.cells()[0]);
        let nb_before = notebook_content_hash(&nb);
        let mut meta = BTreeMap::new();
        meta.insert("lang".into(), "rust".into());
        nb.set_metadata(&id, meta).unwrap();
        assert_ne!(cell_before, cell_content_hash(&nb.cells()[0]));
        assert_ne!(nb_before, notebook_content_hash(&nb));
        assert_eq!(nb.cells()[0].metadata().get("lang").map(String::as_str), Some("rust"));
    }

    #[test]
    fn json_roundtrip_preserves_ids_order_sources_metadata() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        let id = nb.cells()[0].id();
        let mut meta = BTreeMap::new();
        meta.insert("a".into(), "b".into());
        nb.set_metadata(&id, meta).unwrap();
        nb.append(Cell::new_markdown("2".to_string()));
        let json = serde_json::to_string(&nb).unwrap();
        let nb2: Notebook = serde_json::from_str(&json).unwrap();
        assert_eq!(nb.cells().len(), nb2.cells().len());
        for (cell1, cell2) in nb.iter().zip(nb2.iter()) {
            assert_eq!(cell1.id(), cell2.id());
            assert_eq!(cell1.kind(), cell2.kind());
            assert_eq!(cell1.source(), cell2.source());
            assert_eq!(cell1.metadata(), cell2.metadata());
        }
    }
}
