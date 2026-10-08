//! The core module defines the central domain models and traits used throughout Pinner.
//!
//! It is strictly decoupled from side effects (like I/O or network), making it
//! easy to reason about and test the core logic.

pub mod dependency;
pub mod update;

pub use dependency::{
    is_git_sha, is_hash_ref, is_immutable_ref, is_oci_digest, BranchName, CiProvider,
    DependencyName, DependencyRef,
};
pub use update::{
    CompromisedDependency, JsonOutput, NonVettedDependency, UnpinnedDependency, UnsignedDependency,
    UpdateResult, UpdateTask, VerificationResult, VulnerableDependency,
};
