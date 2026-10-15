//! Inspect selected Kotlin grammar nodes using a synthetic input file.
use kode_core::{DocumentId, DocumentOrigin, WorkspaceGeneration, document::Document};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{parser::ParserSession, registry::GrammarRegistry};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("expected synthetic source path")?;
    let text = std::fs::read_to_string(path)?;
    let doc = Document::new(
        DocumentId::fresh()?,
        DocumentOrigin::Virtual {
            owner: "grammar-inspection".into(),
            key: "kotlin".into(),
            label: "synthetic".into(),
        },
        Some("kotlin".into()),
        &text,
    );
    let tree = ParserSession::new(
        GrammarRegistry::compiled()
            .get("kotlin")
            .ok_or("grammar missing")?,
    )?
    .parse(
        &doc.snapshot(),
        WorkspaceGeneration(1),
        &CancellationToken::new(),
    )?;
    println!("{}", tree.root_node().to_sexp());
    Ok(())
}
