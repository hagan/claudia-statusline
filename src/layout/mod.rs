//! Layout rendering module for customizable statusline format.
//!
//! This module provides template-based rendering of the statusline,
//! allowing users to customize the format and order of components.

mod catalog;
mod format;
mod presets;
mod template;
mod variables;

#[cfg(test)]
mod tests;

// Re-exports: public API surface matches pre-split layout.rs
// Note: allow(unused_imports) needed because these are used by lib consumers but not the binary target
#[allow(unused_imports)]
// `user_preset_is_usable` is the loadability predicate `config validate` uses
// to decide `layout.preset` legality by the renderer's OWN resolution
// (plan 12-05); `presets` is a private module, so it must be re-exported here.
pub use presets::{get_preset_format, list_available_presets, user_preset_is_usable};
#[allow(unused_imports)]
pub use presets::{
    MAX_USER_PRESET_BYTES, PRESET_COMPACT, PRESET_DEFAULT, PRESET_DETAILED, PRESET_MINIMAL,
    PRESET_POWER,
};
// The static render-variable catalog: `--list-vars` (binary) and library
// consumers read it.
#[allow(unused_imports)]
pub use catalog::{RenderVar, RENDER_VARIABLES};
pub use template::LayoutRenderer;
pub use variables::{ApiEquivTokens, VariableBuilder};
