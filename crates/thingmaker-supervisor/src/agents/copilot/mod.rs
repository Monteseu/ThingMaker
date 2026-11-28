//! GitHub Copilot CLI speaks ACP directly; no event bridge is needed.

pub mod launch;
pub mod probe;
pub mod quota;

pub use launch::CopilotLaunchOptions;
