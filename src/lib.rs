use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use uuid::{self, Uuid};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellKind {
    Code,
    Markdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    id: CellId,
    kind: CellKind,
    source: String,
    metadata: BTreeMap<String, String>,
}

impl Cell {
    pub fn new_code(source: String) -> Self {
        Cell {
            id: CellId(uuid::Uuid::new_v4()),
            kind: CellKind::Code,
            source,
            metadata: BTreeMap::new(),
        }
    }

    pub fn new_markdown(source: String) -> Self {
        Cell {
            id: CellId(uuid::Uuid::new_v4()),
            kind: CellKind::Markdown,
            source,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notebook {
    cells: Vec<Cell>,
}

impl Default for Notebook {
    fn default() -> Self {
        Self::new()
    }
}

impl Notebook {
    pub fn new() -> Self {
        Notebook { cells: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
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

    pub fn set_metadata(&mut self, id: &CellId, metadata: BTreeMap<String, String>) -> Result<(), NotebookError> {
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

pub fn cell_content_hash(cell: &Cell) -> String {
    let mut hasher = Hasher::new();
    hasher.update(cell.source.as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub fn notebook_content_hash(nb: &Notebook) -> String {
    let mut hasher = Hasher::new();
    for cell in &nb.cells {
        hasher.update(cell.id.0.as_bytes());
        hasher.update(b"\0");
        hasher.update(cell.source.as_bytes());
        hasher.update(b"\0");
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
    }

    #[test]
    fn append_two() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        nb.append(Cell::new_markdown("2".to_string()));
        assert_eq!(nb.len(), 2);
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
        nb.remove(&CellId::from(uuid::Uuid::new_v4())).unwrap_err();
        let id0 = nb.cells[0].id;
        nb.remove(&id0).unwrap();
        assert_eq!(nb.len(), 1);
    }

    #[test]
    fn move_cell() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        nb.append(Cell::new_markdown("2".to_string()));
        nb.move_cell(0, 1).unwrap();
        assert_eq!(nb.cells[0].kind, CellKind::Markdown);
        assert_eq!(nb.cells[1].kind, CellKind::Code);
    }

    #[test]
    fn set_source_changes_hash() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        let old_hash = notebook_content_hash(&nb);
        let id = nb.cells[0].id;
        nb.set_source(&id, "2".to_string()).unwrap();
        let new_hash = notebook_content_hash(&nb);
        assert_ne!(old_hash, new_hash);
    }

    #[test]
    fn json_roundtrip_preserves_ids_order_sources() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("1".to_string()));
        nb.append(Cell::new_markdown("2".to_string()));
        let json = serde_json::to_string(&nb).unwrap();
        let nb2: Notebook = serde_json::from_str(&json).unwrap();
        assert_eq!(nb.cells.len(), nb2.cells.len());
        for (cell1, cell2) in nb.cells.iter().zip(nb2.cells.iter()) {
            assert_eq!(cell1.id, cell2.id);
            assert_eq!(cell1.kind, cell2.kind);
            assert_eq!(cell1.source, cell2.source);
        }
    }
}
