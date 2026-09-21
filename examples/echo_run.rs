//! Load/create a notebook, Explicit intake, run with EchoEngine, print chrome.
use accu_book::{
    save_notebook, Cell, CellChrome, EchoEngine, IntakePolicy, Session,
};
use std::path::PathBuf;

fn main() {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("demo.accu"));

    let mut session = Session::new(EchoEngine);
    assert_eq!(session.intake_policy(), IntakePolicy::Explicit);

    session.edit(|nb| {
        nb.append(Cell::new_markdown("# echo demo".into()));
        nb.append(Cell::new_code("1+1".into()));
    });

    let ids: Vec<_> = session.notebook().cell_ids();
    for id in &ids {
        // Explicit: declare deps (empty = no upstream cells).
        session.set_dependencies(*id, []).expect("declare");
        session.enqueue(*id).expect("enqueue");
    }
    session.run_all();

    for cell in session.notebook().iter() {
        let id = cell.id();
        let chrome = session.chrome(&id);
        let first = cell.source().lines().next().unwrap_or("");
        println!("{id}\t{chrome}\t{first}");
        assert!(
            matches!(
                chrome,
                CellChrome::Ready | CellChrome::Idle | CellChrome::Failed { .. }
            ),
            "unexpected chrome after run_all: {chrome}"
        );
    }

    save_notebook(&path, session.notebook()).expect("save");
    println!("wrote {}", path.display());
}
