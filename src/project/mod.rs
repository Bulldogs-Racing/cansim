//! Versioned, human-readable project format (PROMPT.md §20–§21).
//!
//! YAML (or JSON) on disk, validated before anything runs. Project files are
//! treated as **untrusted input**: paths, component types, firmware refs,
//! backend ids, and numeric values are all checked; no shell commands are
//! ever executed from project content; path traversal is rejected.

pub mod schema;
pub mod validation;

pub use schema::{
    BusDecl, CanAttachment, MessageDecl, NodeDecl, Project, ProjectError, SimulationDecl,
};
pub use validation::{validate_project, ValidationIssue, ValidationReport};
