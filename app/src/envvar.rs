//! Pathfinder's environment variables are named `PATHFINDER_*`. The
//! `HARNESS_*` names from before 0.3 still work.

use std::env::VarError;
use std::ffi::OsString;

fn legacy(name: &str) -> Option<String> {
    name.strip_prefix("PATHFINDER_")
        .map(|rest| format!("HARNESS_{rest}"))
}

/// Like `std::env::var`, with the old name as a fallback.
pub fn get(name: &str) -> Result<String, VarError> {
    match std::env::var(name) {
        Err(VarError::NotPresent) => match legacy(name) {
            Some(old) => std::env::var(old),
            None => Err(VarError::NotPresent),
        },
        other => other,
    }
}

/// Like `std::env::var_os`, with the old name as a fallback.
pub fn get_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).or_else(|| legacy(name).and_then(std::env::var_os))
}
