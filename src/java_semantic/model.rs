use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaSymbolKind {
    Type,
    Method,
    Constructor,
    Field,
    Local,
    Parameter,
    Annotation,
    SyntheticMethod,
}

impl JavaSymbolKind {
    pub const fn public_kind(self) -> &'static str {
        match self {
            Self::Type => "class",
            Self::Method | Self::SyntheticMethod => "function",
            Self::Constructor => "constructor",
            Self::Field => "field",
            Self::Local => "local",
            Self::Parameter => "parameter",
            Self::Annotation => "annotation",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolOrigin {
    Source,
    Scip,
    Classfile,
    GeneratedSource,
    LombokSynthetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveConfidence {
    Scip,
    SourceResolver,
    GeneratedSource,
    ClassfileSummary,
    SyntheticAnnotationModel,
    SyntaxOnly,
    Unresolved,
    Ambiguous,
    IncompleteGeneratedSemantics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResolveStatus {
    Resolved,
    Ambiguous,
    Unresolved,
    IncompleteGeneratedSemantics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceRole {
    Definition,
    Reference,
    Call,
    TypeUse,
    Import,
    Annotation,
    Extends,
    Implements,
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchKind {
    Static,
    Virtual,
    Interface,
    Constructor,
    Super,
    MethodReference,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

impl SourceRange {
    pub const fn new(start_line: u32, start_column: u32, end_line: u32, end_column: u32) -> Self {
        Self {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    pub fn contains(&self, other: &Self) -> bool {
        (self.start_line, self.start_column) <= (other.start_line, other.start_column)
            && (self.end_line, self.end_column) >= (other.end_line, other.end_column)
    }

    pub fn to_codetrail_json(&self) -> Value {
        json!({
            "start": { "line": self.start_line, "column": self.start_column },
            "end": { "line": self.end_line, "column": self.end_column }
        })
    }

    pub fn to_lsp_json(&self) -> Value {
        json!({
            "start": { "line": self.start_line, "character": self.start_column },
            "end": { "line": self.end_line, "character": self.end_column }
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaSymbol {
    pub symbol_id: String,
    pub name: String,
    pub kind: JavaSymbolKind,
    pub package: String,
    pub qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<SourceRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_range: Option<SourceRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<String>,
    pub origin: SymbolOrigin,
    pub confidence: ResolveConfidence,
    #[serde(default)]
    pub root_id: String,
    #[serde(default)]
    pub file_hash: String,
}

impl JavaSymbol {
    pub fn display_signature(&self) -> String {
        match self.kind {
            JavaSymbolKind::Method | JavaSymbolKind::SyntheticMethod => {
                let owner = self
                    .qualified_name
                    .split('#')
                    .next()
                    .unwrap_or(&self.qualified_name);
                let params = self.parameters.join(", ");
                format!("{owner}.{}({params})", self.name)
            }
            JavaSymbolKind::Constructor => {
                let owner = self
                    .qualified_name
                    .split('#')
                    .next()
                    .unwrap_or(&self.qualified_name);
                let params = self.parameters.join(", ");
                format!("{owner}.{}({params})", self.name)
            }
            JavaSymbolKind::Field => self
                .return_type
                .as_ref()
                .map(|ty| format!("{}: {ty}", self.qualified_name))
                .unwrap_or_else(|| self.qualified_name.clone()),
            _ => self.qualified_name.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaOccurrence {
    pub path: String,
    pub range: SourceRange,
    pub role: OccurrenceRole,
    pub symbol_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosing_symbol: Option<String>,
    pub syntax_kind: String,
    pub source: String,
    pub confidence: ResolveConfidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaCallEdge {
    pub caller_symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callee_symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub possible_callees: Vec<String>,
    pub target_name: String,
    pub path: String,
    pub range: SourceRange,
    pub file_hash: String,
    pub dispatch_kind: DispatchKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<String>,
    pub status: ResolveStatus,
    pub confidence: ResolveConfidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaTypeEdge {
    pub subtype: String,
    pub supertype: String,
    pub relation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaFileContribution {
    pub path: String,
    pub file_hash: String,
    pub symbol_count: usize,
    pub occurrence_count: usize,
    pub call_edge_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaSemanticManifest {
    pub schema_version: u32,
    pub tool_version: String,
    pub snapshot_id: String,
    pub snapshot_key: String,
    pub source: String,
    pub file_count: usize,
    pub symbol_count: usize,
    pub occurrence_count: usize,
    pub call_edge_count: usize,
    pub type_edge_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaSemanticData {
    pub manifest: JavaSemanticManifest,
    pub symbols: Vec<JavaSymbol>,
    pub occurrences: Vec<JavaOccurrence>,
    pub call_edges: Vec<JavaCallEdge>,
    pub type_edges: Vec<JavaTypeEdge>,
    pub file_contributions: Vec<JavaFileContribution>,
}

#[derive(Clone, Debug)]
pub struct ExtractedJavaFile {
    pub path: String,
    pub root_id: String,
    pub file_hash: String,
    pub package: String,
    pub imports: Vec<JavaImport>,
    pub symbols: Vec<JavaSymbol>,
    pub raw_calls: Vec<RawJavaCall>,
    pub type_edges: Vec<JavaTypeEdge>,
    pub annotations: Vec<JavaAnnotation>,
    pub generated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JavaImport {
    pub path: String,
    pub is_static: bool,
    pub is_wildcard: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JavaAnnotation {
    pub name: String,
    pub owner_symbol: String,
}

#[derive(Clone, Debug)]
pub struct RawJavaCall {
    pub path: String,
    pub file_hash: String,
    pub caller_symbol: String,
    pub target_name: String,
    pub receiver_text: Option<String>,
    pub receiver_type: Option<String>,
    pub arg_count: usize,
    pub range: SourceRange,
    pub dispatch_kind: DispatchKind,
}
