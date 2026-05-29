use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use prost::Message;

use crate::scip_proto::proto;

/// Parse a native SCIP binary protobuf file (index.scip).
pub fn parse_native_scip(path: &Path) -> Result<proto::Index> {
    let data = fs::read(path)
        .with_context(|| format!("failed to read SCIP index file {}", path.display()))?;
    parse_native_scip_from_bytes(&data)
}

/// Parse SCIP protobuf from raw bytes.
pub fn parse_native_scip_from_bytes(data: &[u8]) -> Result<proto::Index> {
    proto::Index::decode(data).with_context(|| "failed to decode SCIP index protobuf")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal SCIP index protobuf and verify round-trip decode.
    #[test]
    fn round_trip_minimal_index() {
        let index = proto::Index {
            metadata: Some(proto::Metadata {
                version: proto::ProtocolVersion::UnspecifiedProtocolVersion as i32,
                tool_info: Some(proto::ToolInfo {
                    name: "test-indexer".to_string(),
                    version: "0.1.0".to_string(),
                    arguments: vec![],
                }),
                project_root: "file:///test".to_string(),
                text_document_encoding: proto::TextEncoding::Utf8 as i32,
            }),
            documents: vec![proto::Document {
                language: "rust".to_string(),
                relative_path: "src/lib.rs".to_string(),
                occurrences: vec![proto::Occurrence {
                    range: vec![0, 3, 0, 9],
                    symbol: "local 1".to_string(),
                    symbol_roles: 1, // Definition
                    syntax_kind: proto::SyntaxKind::IdentifierFunctionDefinition as i32,
                    ..Default::default()
                }],
                symbols: vec![proto::SymbolInformation {
                    symbol: "local 1".to_string(),
                    kind: proto::symbol_information::Kind::Function as i32,
                    display_name: "needle".to_string(),
                    ..Default::default()
                }],
                position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut buf = Vec::new();
        index.encode(&mut buf).unwrap();
        let decoded = parse_native_scip_from_bytes(&buf).unwrap();

        assert_eq!(decoded.documents.len(), 1);
        assert_eq!(decoded.documents[0].relative_path, "src/lib.rs");
        assert_eq!(decoded.documents[0].occurrences.len(), 1);
        assert_eq!(decoded.documents[0].occurrences[0].symbol_roles, 1);
        assert_eq!(decoded.documents[0].symbols.len(), 1);
        assert_eq!(decoded.documents[0].symbols[0].display_name, "needle");
    }
}
