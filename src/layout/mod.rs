//! Layout rendering module for customizable statusline format.
//!
//! This module provides template-based rendering of the statusline,
//! allowing users to customize the format and order of components.

mod format;
mod presets;
mod template;
mod variables;

#[cfg(test)]
mod tests;

// Re-exports: public API surface matches pre-split layout.rs
// Note: allow(unused_imports) needed because these are used by lib consumers but not the binary target
#[allow(unused_imports)]
pub use presets::{get_preset_format, list_available_presets};
#[allow(unused_imports)]
pub use presets::{PRESET_COMPACT, PRESET_DEFAULT, PRESET_DETAILED, PRESET_MINIMAL, PRESET_POWER};
pub use template::LayoutRenderer;
pub use variables::VariableBuilder;
