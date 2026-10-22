//! Single-output ingress processing. The caller supplies the current user's home,
//! not an independently configurable original-output directory.

pub mod bridge;
pub mod command;
pub mod execution;
pub mod gain;
pub mod install;
mod processing;
pub mod protocol;
mod recording;
pub mod rewrite;
mod rules;
mod storage;

pub use processing::Processor;
