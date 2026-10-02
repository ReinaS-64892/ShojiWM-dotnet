//! Reused .NET hosting and managed-generation machinery; no compositor state.
pub(crate) mod assembly;
pub(crate) mod evaluator;
mod host;
mod process;
mod protocol;
pub(crate) mod source;
