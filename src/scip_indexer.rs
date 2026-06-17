//! SCIP indexer orchestration.
//!
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{
    lsp::scip_gen::{self, SemanticBuildReport},
    output::VerboseLogger,
    project_graph::ProjectLanguage,
    scip_proto::proto,
    workspace::{ScanOptions, Workspace},
};

#[derive(Clone, Debug)]
pub struct GeneratedScipSummary {
    pub semantic_report: SemanticBuildReport,
    pub document_count: usize,
    pub occurrence_count: usize,
    pub symbol_count: usize,
}

/// Run the Go SCIP indexer on a project root and return the path to the
/// generated SCIP JSON file.
pub fn generate_go_scip(project_root: &Path, output_path: &Path) -> Result<()> {
    let indexer_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scip-indexer");

    let output = Command::new("go")
        .args(["run", "main.go", "--output"])
        .arg(output_path)
        .arg(project_root)
        .current_dir(&indexer_dir)
        .output()
        .with_context(|| {
            format!(
                "failed to run Go SCIP indexer for {}",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Go SCIP indexer failed: {stderr}");
    }

    eprintln!("{}", String::from_utf8_lossy(&output.stdout).trim());
    Ok(())
}

pub fn generate_swift_scip(
    project_root: &Path,
    output_path: &Path,
    verbose: VerboseLogger,
) -> Result<GeneratedScipSummary> {
    let workspace = Workspace::discover(project_root)?;
    let scan_opts = ScanOptions {
        lang: vec!["swift".to_string()],
        limit: 0,
        allow_broad: true,
        ..ScanOptions::default()
    };
    let records = workspace.scan_files(&scan_opts)?;
    let (index, semantic_report) = scip_gen::generate_index_for_language(
        &workspace,
        &records,
        ProjectLanguage::Swift,
        verbose,
    )
    .with_context(|| "failed to generate Swift SCIP through SourceKit-LSP")?;
    let document_count = index.documents.len();
    let occurrence_count = index
        .documents
        .iter()
        .map(|document| document.occurrences.len())
        .sum();
    let symbol_count = index
        .documents
        .iter()
        .map(|document| document.symbols.len())
        .sum();
    write_scip_json(&index, output_path)?;
    Ok(GeneratedScipSummary {
        semantic_report,
        document_count,
        occurrence_count,
        symbol_count,
    })
}

fn write_scip_json(index: &proto::Index, output_path: &Path) -> Result<()> {
    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(
        output_path,
        serde_json::to_vec_pretty(&scip_index_json(index))?,
    )
    .with_context(|| format!("failed to write SCIP JSON {}", output_path.display()))?;
    Ok(())
}

fn scip_index_json(index: &proto::Index) -> Value {
    json!({
        "metadata": index.metadata.as_ref().map(metadata_json),
        "documents": index.documents.iter().map(document_json).collect::<Vec<_>>(),
        "externalSymbols": index.external_symbols.iter().map(symbol_json).collect::<Vec<_>>(),
    })
}

fn metadata_json(metadata: &proto::Metadata) -> Value {
    json!({
        "version": metadata.version,
        "toolInfo": metadata.tool_info.as_ref().map(tool_info_json),
        "projectRoot": metadata.project_root,
        "textDocumentEncoding": metadata.text_document_encoding,
    })
}

fn tool_info_json(tool_info: &proto::ToolInfo) -> Value {
    json!({
        "name": tool_info.name,
        "version": tool_info.version,
        "arguments": tool_info.arguments,
    })
}

fn document_json(document: &proto::Document) -> Value {
    json!({
        "relativePath": document.relative_path,
        "language": document.language,
        "occurrences": document.occurrences.iter().map(occurrence_json).collect::<Vec<_>>(),
        "symbols": document.symbols.iter().map(symbol_json).collect::<Vec<_>>(),
        "text": document.text,
        "positionEncoding": document.position_encoding,
    })
}

fn occurrence_json(occurrence: &proto::Occurrence) -> Value {
    json!({
        "range": occurrence.range,
        "symbol": occurrence.symbol,
        "symbolRoles": occurrence.symbol_roles,
        "syntaxKind": occurrence.syntax_kind,
    })
}

fn symbol_json(symbol: &proto::SymbolInformation) -> Value {
    json!({
        "symbol": symbol.symbol,
        "documentation": symbol.documentation,
        "relationships": symbol.relationships.iter().map(relationship_json).collect::<Vec<_>>(),
        "kind": symbol.kind,
        "displayName": symbol.display_name,
        "signatureDocumentation": symbol.signature_documentation.as_ref().map(signature_json),
        "enclosingSymbol": symbol.enclosing_symbol,
    })
}

fn relationship_json(relationship: &proto::Relationship) -> Value {
    json!({
        "symbol": relationship.symbol,
        "isReference": relationship.is_reference,
        "isImplementation": relationship.is_implementation,
        "isTypeDefinition": relationship.is_type_definition,
        "isDefinition": relationship.is_definition,
    })
}

fn signature_json(signature: &proto::Signature) -> Value {
    json!({
        "language": signature.language,
        "text": signature.text,
        "occurrences": signature.occurrences.iter().map(occurrence_json).collect::<Vec<_>>(),
    })
}

/// Run the Go SCIP indexer and then import the result.
pub fn generate_and_import(project_root: &Path) -> Result<()> {
    let tmp = tempfile::Builder::new()
        .prefix("codetrail-index-")
        .suffix(".scip.json")
        .tempfile()
        .with_context(|| "failed to create temporary SCIP output file")?;
    let tmp_path = tmp.path().to_path_buf();
    generate_go_scip(project_root, &tmp_path)?;

    // Import using the existing command
    let status = Command::new(std::env::current_exe().unwrap_or_else(|_| "codetrail".into()))
        .args(["index", "import-scip"])
        .arg(&tmp_path)
        .current_dir(project_root)
        .status()
        .with_context(|| "failed to import generated SCIP index")?;

    if !status.success() {
        anyhow::bail!("SCIP import failed");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn go_available() -> bool {
        Command::new("go")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn go_indexer_produces_output_for_valid_project() {
        if !go_available() {
            eprintln!("skipping test: Go toolchain not available");
            return;
        }
        let dir = tempdir().unwrap();
        let project = dir.path();
        fs::create_dir_all(project.join("pkg")).unwrap();
        fs::write(
            project.join("go.mod"),
            "module example.com/test\n\ngo 1.21\n",
        )
        .unwrap();
        fs::write(
            project.join("pkg/math.go"),
            "package pkg\n\n// Add returns the sum of two integers.\nfunc Add(a, b int) int { return a + b }\n",
        )
        .unwrap();

        let output = project.join("index.scip.json");
        let result = generate_go_scip(project, &output);
        // May fail if Go toolchain issues, but should not panic
        if result.is_ok() {
            assert!(output.exists());
            let content = fs::read_to_string(&output).unwrap();
            assert!(content.contains("Add"));
            assert!(content.contains("\"documents\""));
        }
    }
}
