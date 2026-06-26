//! Rust-native Java semantic index.
//!
//! This provider intentionally stays inside the codetrail process: tree-sitter
//! supplies syntax, SCIP occurrence data can be merged when available, and the
//! lightweight resolver/index below provides navigation evidence for Java call
//! hierarchy queries.

pub mod classfile;
pub mod extract;
pub mod hierarchy;
pub mod index;
pub mod lombok;
pub mod model;
pub mod parse;
pub mod resolver;
pub mod store;

pub use hierarchy::{CallHierarchyDirection, CallHierarchyOptions};
pub use index::{
    build, callers, calls, index_meta, is_fresh, query_call_hierarchy, JavaSemanticBuildReport,
};
