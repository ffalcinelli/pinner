//! The core module defines the central domain models and traits used throughout Pinner.
//!
//! It is strictly decoupled from side effects (like I/O or network), making it
//! easy to reason about and test the core logic.

pub mod dependency;
pub mod update;

pub use dependency::{BranchName, CiProvider, DependencyName, DependencyRef};
pub use update::{
    CompromisedDependency, JsonOutput, NonVettedDependency, UnpinnedDependency, UpdateResult,
    UpdateTask, VerificationResult,
};
