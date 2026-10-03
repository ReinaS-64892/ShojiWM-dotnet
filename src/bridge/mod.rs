//! Reused .NET hosting and managed-generation machinery; no compositor state.
mod arena;
pub(crate) mod assembly;
pub(crate) mod evaluator;
mod host;
mod native;
mod native_generated;
mod process;
pub(crate) mod source;
