//! The patcher module is responsible for applying identified dependency updates
//! to the source files while strictly preserving formatting and comments.
//!
//! It consists of:
//! - `mutator`: Low-level string manipulation logic.
//! - `disk`: High-level orchestration of file I/O and patch calculation.
//! - `formatter`: Pretty-printing of diffs and results.
//! - `report`: Rendering of `verify` results in every output format.
//! - `ui`: Interaction with the user for confirmations.

pub mod disk;
pub mod formatter;
pub mod mutator;
pub mod report;
pub mod ui;

pub use disk::Patcher;
pub use formatter::Formatter;
pub use ui::{ConsoleUi, UserInterface};
