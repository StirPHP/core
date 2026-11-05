//! PHPStan next to our own analysis: finding a project's PHPStan, running it in the background, and
//! showing what it reports with our diagnostics, without the same error twice.
//!
//! Native only, so `stir_wasm` never depends on this crate. Running PHPStan runs the project's
//! code: whoever calls [`run`] has to have decided that's all right.

mod find;
mod merge;
mod report;
mod run;

pub use find::{Invocation, Located, Wanted, locate};
pub use merge::{diagnostics, merge};
pub use report::{Report, Reported, is_incomplete, is_memory, parse};
pub use run::{Failure, kill_all, run, signalled};
