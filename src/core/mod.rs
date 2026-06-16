pub mod dependency;
pub mod update;

pub use dependency::{BranchName, CiProvider, DependencyName, DependencyRef};
pub use update::{JsonOutput, UnpinnedDependency, UpdateResult, UpdateTask, VerificationResult};
