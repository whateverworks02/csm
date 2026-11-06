//! csm library: the workspace-memory engine behind the `csm` binary.

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
