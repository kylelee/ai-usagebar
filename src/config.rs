//! Config file at `~/.config/ai-usagebar-omarchy/config.toml` (pre-rename `~/.config/ai-usagebar/` is migrated on first run — see `migrate_legacy_layout`).
//!
//! Layout:
//! ```toml
//! [anthropic]  enabled = true
//! [openai]     enabled = true   # Codex OAuth from ~/.codex/auth.json
//! [zai]        enabled = true
//! [openrouter] enabled = true
//! [deepseek]   enabled = false
//! [kimi]       enabled = false
//! ```
//!
//! Every field is optional with sensible defaults — a missing config file is
//! treated as "use defaults", except that the first run drops the annotated
//! [`EXAMPLE_CONFIG`] template at the canonical path so there is a commented
//! file to fill in (see [`seed_example_config`]; best-effort). API keys are
//! read from env vars (the relevant `*_api_key_env` field lets the user
//! override which env var name).
//!
//! This module is the root of the `config/` directory: the section structs
//! live in [`sections`], account types and lookups in [`accounts`], the
//! per-vendor field dispatchers in [`dispatch`], loading/validation in
//! [`loader`], the seed template in [`template`], and the pre-rename layout
//! migration in [`migrate`]. Every public item is re-exported here, so
//! `crate::config::X` paths are unchanged.

mod accounts;
mod dispatch;
mod loader;
mod migrate;
mod sections;
mod template;

#[cfg(test)]
mod tests;

pub use accounts::{
    AnthropicAccount, KeyAccount, KimiAccount, OpenAiAccount, OpenRouterAccount,
    add_anthropic_account_to_doc, default_account_credentials_path, tildify,
    validate_account_label,
};
pub use loader::{optional_api_key, resolve_api_key};
pub use migrate::{default_path, migrate_legacy_layout, resolved_path};
pub use sections::{
    AnthropicApiConfig, AnthropicConfig, AntigravityConfig, Config, ContextConfig, ContextLayout,
    CursorConfig, DeepseekConfig, GrokConfig, KiloConfig, KimiConfig, KiroConfig, MinimaxConfig,
    MoonshotConfig, NovitaConfig, OpenAiConfig, OpenCodeGoConfig, OpenRouterConfig,
    OpenaiApiConfig, ResolvedZaiAccount, SuperGrokConfig, UiConfig, VendorBoxStyle, ZaiAccount,
    ZaiAccountType, ZaiConfig, ZaiSite,
};
pub use template::{EXAMPLE_CONFIG, config_path_hint, seed_example_config};

pub(crate) use loader::resolve_key_account;

// Seams the embedded test module reaches through `use super::*` — the same
// top-level `use` bindings the pre-split single file provided. A glob import
// does not count as usage for `unused_imports`, so only names the tests use
// bare (never fully qualified) are bound here.
#[cfg(test)]
use crate::anthropic::creds::CredsTarget;
#[cfg(test)]
use crate::vendor::VendorId;
#[cfg(test)]
use std::path::{Path, PathBuf};

// Cross-submodule + test seams, named here as privately as in the pre-split
// single file: definitions are `pub(super)` (invisible outside `config`),
// and these root bindings are plain private `use`s — a `pub` re-export of
// them would widen visibility and fail E0364.
#[cfg(test)]
#[cfg(unix)]
use dispatch::{InlineKeyPermissionDecision, inline_key_permission_decision};
#[cfg(test)]
use loader::is_valid_env_var_name;
use loader::resolve_api_key_in_section;
#[cfg(test)]
use migrate::{copy_dir_recursively, migrate_dir_at, pre_rename_config_paths};
#[cfg(test)]
use sections::default_grok_binary;
