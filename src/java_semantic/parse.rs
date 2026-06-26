use tree_sitter::{Node, Point};

use crate::java_semantic::model::SourceRange;

pub(crate) fn node_text(node: Node, source: &[u8]) -> Option<String> {
    node.utf8_text(source)
        .ok()
        .map(|value| value.trim().to_string())
}

pub(crate) fn point_range(node: Node) -> SourceRange {
    range_from_points(node.start_position(), node.end_position())
}

pub(crate) fn range_from_points(start: Point, end: Point) -> SourceRange {
    SourceRange::new(
        start.row as u32 + 1,
        start.column as u32 + 1,
        end.row as u32 + 1,
        end.column as u32 + 1,
    )
}

pub(crate) fn child_text(node: Node, field: &str, source: &[u8]) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|child| node_text(child, source))
}

pub(crate) fn child_by_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    found
}

pub(crate) fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

pub(crate) fn last_identifier(value: &str) -> String {
    value
        .rsplit(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .find(|part| !part.is_empty())
        .unwrap_or(value)
        .to_string()
}

pub(crate) fn erase_type(value: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in value.trim().chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            '[' if depth == 0 => break,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}
