//! LSP client bridge for semantic indexing during `index build`.
//!
//! Spawns language servers over stdio, collects `documentSymbol` / `references`,
//! and synthesizes SCIP occurrence databases.

pub mod client;
pub mod provider;
pub mod registry;
pub mod scip_gen;
pub mod transport;

pub use scip_gen::{generate_best_effort, SemanticBuildReport};
