//! csm library: the workspace-memory engine behind the `csm` binary, shared
//! with the dev-side `csm-eval` instrument so both run the same store,
//! workspace, and deploy code paths.

pub mod agent;
pub mod doctor;
pub mod gc;
pub mod hook;
pub mod inject;
pub mod markdown;
pub mod needs;
pub mod prompt;
pub mod skills;
pub mod store;
pub mod ui;
pub mod workspace;

#[cfg(test)]
pub mod test_support;
