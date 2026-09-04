use std::path::Path;

use super::{Config, KeyAccount};
use crate::error::{AppError, Result};
use crate::vendor::VendorId;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// One API-key vendor's section fields, unified so every per-vendor dispatcher
/// enumerates the same set (canonical `VendorId` order). `accounts` is `None`
/// where the numbered entries are not `KeyAccount`-shaped: zai carries its own
/// richer `[[zai.accounts]]` type and participates in the key/env-name
/// lookups only.
struct KeySection<'a> {
    vendor: VendorId,
    api_key_env: &'a str,
    api_key: Option<&'a str>,
    accounts: Option<&'a [KeyAccount]>,
    show_default_account: bool,
    enabled: bool,
}

impl Config {
    /// The single enumeration of API-key vendor sections. Adding a key vendor
    /// here updates every consumer below (and `derive_account_labels` via its
    /// mutable twin) at once.
    fn key_sections(&self) -> impl Iterator<Item = KeySection<'_>> {
        [
            KeySection {
                vendor: VendorId::AnthropicApi,
                api_key_env: &self.anthropic_api.api_key_env,
                api_key: self.anthropic_api.api_key.as_deref(),
                accounts: Some(&self.anthropic_api.accounts),
                show_default_account: self.anthropic_api.show_default_account,
                enabled: self.anthropic_api.enabled,
            },
            KeySection {
                vendor: VendorId::OpenaiApi,
                api_key_env: &self.openai_api.api_key_env,
                api_key: self.openai_api.api_key.as_deref(),
                accounts: Some(&self.openai_api.accounts),
                show_default_account: self.openai_api.show_default_account,
                enabled: self.openai_api.enabled,
            },
            KeySection {
                vendor: VendorId::Zai,
                api_key_env: &self.zai.api_key_env,
                api_key: self.zai.api_key.as_deref(),
                accounts: None,
                show_default_account: self.zai.show_default_account,
                enabled: self.zai.enabled,
            },
            KeySection {
                vendor: VendorId::Openrouter,
                api_key_env: &self.openrouter.api_key_env,
                api_key: self.openrouter.api_key.as_deref(),
                accounts: Some(&self.openrouter.accounts),
                show_default_account: self.openrouter.show_default_account,
                enabled: self.openrouter.enabled,
            },
            KeySection {
                vendor: VendorId::Deepseek,
                api_key_env: &self.deepseek.api_key_env,
                api_key: self.deepseek.api_key.as_deref(),
                accounts: Some(&self.deepseek.accounts),
                show_default_account: self.deepseek.show_default_account,
                enabled: self.deepseek.enabled,
            },
            KeySection {
                vendor: VendorId::Kimi,
                api_key_env: &self.kimi.api_key_env,
                api_key: self.kimi.api_key.as_deref(),
                accounts: Some(&self.kimi.accounts),
                show_default_account: self.kimi.show_default_account,
                enabled: self.kimi.enabled,
            },
            KeySection {
                vendor: VendorId::Kilo,
                api_key_env: &self.kilo.api_key_env,
                api_key: self.kilo.api_key.as_deref(),
                accounts: Some(&self.kilo.accounts),
                show_default_account: self.kilo.show_default_account,
                enabled: self.kilo.enabled,
            },
            KeySection {
                vendor: VendorId::Novita,
                api_key_env: &self.novita.api_key_env,
                api_key: self.novita.api_key.as_deref(),
                accounts: Some(&self.novita.accounts),
                show_default_account: self.novita.show_default_account,
                enabled: self.novita.enabled,
            },
            KeySection {
                vendor: VendorId::Moonshot,
                api_key_env: &self.moonshot.api_key_env,
                api_key: self.moonshot.api_key.as_deref(),
                accounts: Some(&self.moonshot.accounts),
                show_default_account: self.moonshot.show_default_account,
                enabled: self.moonshot.enabled,
            },
            KeySection {
                vendor: VendorId::Grok,
                api_key_env: &self.grok.api_key_env,
                api_key: self.grok.api_key.as_deref(),
                accounts: Some(&self.grok.accounts),
                show_default_account: self.grok.show_default_account,
                enabled: self.grok.enabled,
            },
            KeySection {
                vendor: VendorId::Minimax,
                api_key_env: &self.minimax.api_key_env,
                api_key: self.minimax.api_key.as_deref(),
                accounts: Some(&self.minimax.accounts),
                show_default_account: self.minimax.show_default_account,
                enabled: self.minimax.enabled,
            },
            KeySection {
                vendor: VendorId::OpenCodeGo,
                api_key_env: &self.opencode_go.api_key_env,
                api_key: self.opencode_go.api_key.as_deref(),
                accounts: Some(&self.opencode_go.accounts),
                show_default_account: self.opencode_go.show_default_account,
                enabled: self.opencode_go.enabled,
            },
        ]
        .into_iter()
    }

    /// Mutable twin of [`Config::key_sections`]: the same eleven numbered
    /// `[[vendor.accounts]]` vectors, for in-place relabeling. zai is absent —
    /// its accounts have their own shape and are relabeled separately.
    pub(super) fn key_account_sections_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut Vec<KeyAccount>> {
        [
            &mut self.anthropic_api.accounts,
            &mut self.openai_api.accounts,
            &mut self.openrouter.accounts,
            &mut self.deepseek.accounts,
            &mut self.kimi.accounts,
            &mut self.kilo.accounts,
            &mut self.novita.accounts,
            &mut self.moonshot.accounts,
            &mut self.grok.accounts,
            &mut self.minimax.accounts,
            &mut self.opencode_go.accounts,
        ]
        .into_iter()
    }

    /// The numbered `[[vendor.accounts]]` entries of every key-based vendor
    /// (labels already derived). `None` for vendors whose accounts are not
    /// key-based (zai has its own shape; anthropic/openai are OAuth paths).
    pub(crate) fn key_accounts(&self, vendor: crate::vendor::VendorId) -> Option<&[KeyAccount]> {
        self.key_sections()
            .find(|section| section.vendor == vendor)
            .and_then(|section| section.accounts)
    }

    /// Whether a vendor's aggregate views include its default key when
    /// numbered accounts exist (the kimi rule). Only the `KeyAccount`-shaped
    /// sections dispatch — zai keeps the historical `true` fallback.
    pub(crate) fn key_show_default(&self, vendor: crate::vendor::VendorId) -> bool {
        self.key_sections()
            .find(|section| section.vendor == vendor)
            .filter(|section| section.accounts.is_some())
            .is_none_or(|section| section.show_default_account)
    }

    /// The section-default inline `api_key` for an API-key vendor. The single
    /// source for the TUI settings overlay, the settings bridge, and the tab
    /// builder's credential check — a new key-account vendor added here updates
    /// every consumer at once. `None` for vendors that do not authenticate
    /// with a config-file key. The value is returned raw (an empty inline key
    /// is `Some("")`); callers that need "actually set" filter themselves.
    pub(crate) fn inline_api_key(&self, vendor: VendorId) -> Option<&str> {
        self.key_sections()
            .find(|section| section.vendor == vendor)
            .and_then(|section| section.api_key)
    }

    /// The section-default `api_key_env` variable name for an API-key vendor
    /// (user-customizable per section). Same vendor set as
    /// [`Config::inline_api_key`].
    pub(crate) fn api_key_env_name(&self, vendor: VendorId) -> Option<&str> {
        self.key_sections()
            .find(|section| section.vendor == vendor)
            .map(|section| section.api_key_env)
    }

    /// Every inline API-key field, via the same section enumeration as the
    /// dispatchers above. Section-default keys and every numbered account's
    /// inline key are secrets alike — a `[[deepseek.accounts]]` entry is as
    /// much one as `[deepseek] api_key`. (`[[openai_api.accounts]]` keys are
    /// outside today's protection scope; that predates this refactor and is
    /// preserved verbatim.)
    #[cfg(unix)]
    pub(super) fn has_inline_api_keys(&self) -> bool {
        self.key_sections().any(|section| {
            section.api_key.is_some_and(|key| !key.is_empty())
                || (section.vendor != VendorId::OpenaiApi
                    && section.accounts.is_some_and(|accounts| {
                        accounts.iter().any(|account| {
                            account
                                .api_key
                                .as_deref()
                                .is_some_and(|key| !key.is_empty())
                        })
                    }))
        })
    }

    #[cfg(unix)]
    pub(super) fn protect_inline_api_keys(&self, path: &Path) -> Result<()> {
        if !self.has_inline_api_keys() {
            return Ok(());
        }

        let metadata = std::fs::metadata(path).map_err(|_| {
            AppError::Credentials(format!(
                "config at {} contains inline api_key values but its permissions could not be checked; fix permissions or move keys to environment variables",
                crate::display::sanitize_untrusted_path(path)
            ))
        })?;
        if inline_key_permission_decision(metadata.mode()) == InlineKeyPermissionDecision::Tighten {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|_| {
                AppError::Credentials(format!(
                    "config at {} contains inline api_key values but is group/other-readable and could not be tightened to 0600; fix permissions or move keys to environment variables",
                    crate::display::sanitize_untrusted_path(path)
                ))
            })?;
        }
        Ok(())
    }

    pub fn is_enabled(&self, id: VendorId) -> bool {
        if let Some(section) = self.key_sections().find(|section| section.vendor == id) {
            return section.enabled;
        }
        match id {
            VendorId::Anthropic => self.anthropic.enabled,
            VendorId::Openai => self.openai.enabled,
            VendorId::Supergrok => self.supergrok.enabled,
            VendorId::Antigravity => self.antigravity.enabled,
            VendorId::Cursor => self.cursor.enabled,
            VendorId::Kiro => self.kiro.enabled,
            // Only reachable for a variant not yet registered in
            // `key_sections` above — reads as "not enabled".
            _ => false,
        }
    }

    pub fn enabled_vendors(&self) -> Vec<VendorId> {
        VendorId::all()
            .iter()
            .copied()
            .filter(|id| self.is_enabled(*id))
            .collect()
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InlineKeyPermissionDecision {
    Ok,
    Tighten,
}

#[cfg(unix)]
pub(super) fn inline_key_permission_decision(mode: u32) -> InlineKeyPermissionDecision {
    if mode & 0o077 == 0 {
        InlineKeyPermissionDecision::Ok
    } else {
        InlineKeyPermissionDecision::Tighten
    }
}
