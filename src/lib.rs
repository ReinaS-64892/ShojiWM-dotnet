//! A .NET configuration runtime injected through ShojiWM's public runtime API.
mod bridge;
mod runtime;

pub use runtime::{DotNetLauncher, DotNetRuntime};
pub use shojiwm_lib::run;
