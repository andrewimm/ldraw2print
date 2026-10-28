//! Error kinds that callers (and agents) need to tell apart, mapped to
//! process exit codes. Everything else is a generic failure (exit 1);
//! command-line usage errors exit with 2 (clap's convention).

use std::fmt;

/// Exit codes. Stable: documented in `ldraw2print guide agents`.
pub mod exit {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const NOT_WATERTIGHT: i32 = 3;
    pub const DECISION_NEEDED: i32 = 4;
    pub const MISSING_FILES: i32 = 5;
}

/// The connector surface can't be chosen automatically (none or several
/// match); the caller must pick one with `--connector-candidate`.
#[derive(Debug)]
pub struct DecisionNeeded(pub String);

/// Referenced LDraw files are missing and `--allow-missing` wasn't given.
#[derive(Debug)]
pub struct MissingFiles(pub String);

/// `--strict` was given and the mesh isn't watertight.
#[derive(Debug)]
pub struct NotWatertight(pub String);

macro_rules! impl_err {
    ($t:ty, $kind:literal) => {
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl std::error::Error for $t {}
        impl $t {
            pub const KIND: &'static str = $kind;
        }
    };
}
impl_err!(DecisionNeeded, "decision-needed");
impl_err!(MissingFiles, "missing-files");
impl_err!(NotWatertight, "not-watertight");

/// Exit code and machine-readable kind for an error.
pub fn classify(e: &anyhow::Error) -> (i32, &'static str) {
    for cause in e.chain() {
        if cause.is::<DecisionNeeded>() {
            return (exit::DECISION_NEEDED, DecisionNeeded::KIND);
        }
        if cause.is::<MissingFiles>() {
            return (exit::MISSING_FILES, MissingFiles::KIND);
        }
        if cause.is::<NotWatertight>() {
            return (exit::NOT_WATERTIGHT, NotWatertight::KIND);
        }
    }
    (exit::ERROR, "error")
}
