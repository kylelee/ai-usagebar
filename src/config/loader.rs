use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{
    Config, KeyAccount, config_path_hint, migrate_legacy_layout, resolved_path,
    seed_example_config, validate_account_label,
};
use crate::error::{AppError, Result};

/// Resolve an API key for a vendor: a valid env-var name wins, then inline
/// config, then a clear error naming both fields. Used by every API-key vendor.
pub fn resolve_api_key(
    vendor_label: &str,
    env_var_name: &str,
    inline: Option<&str>,
) -> crate::error::Result<String> {
    let section = match vendor_label {
        "OpenCode Go" => "[opencode-go]".to_string(),
        _ => format!("[{}]", vendor_label.to_lowercase()),
    };
    resolve_api_key_in_section(vendor_label, &section, env_var_name, inline)
}

/// The env-then-inline lookup without the "or fail" ending, for vendors where
/// an absent API key is a legitimate state rather than an error — Kimi accepts
/// a Kimi Code CLI subscription login instead.
pub fn optional_api_key(env_var_name: &str, inline: Option<&str>) -> Option<String> {
    if is_valid_env_var_name(env_var_name)
        && let Ok(v) = std::env::var(env_var_name)
        && !v.is_empty()
    {
        return Some(v);
    }
    inline.filter(|v| !v.is_empty()).map(str::to_string)
}

/// Resolve the key of one numbered `[[vendor.accounts]]` entry: its env var
/// wins, the inline key falls back — exactly the semantics of every vendor's
/// default section, so all key vendors behave identically. `accounts` must
/// be label-derived (post `derive_account_labels`).
pub(crate) fn resolve_key_account(
    vendor_display: &str,
    section: &str,
    accounts: &[KeyAccount],
    account: &str,
) -> Result<String> {
    let entry = accounts
        .iter()
        .find(|candidate| candidate.label == account)
        .ok_or_else(|| {
            AppError::Credentials(format!(
                "{vendor_display} account {account:?} not found in [[{section}.accounts]]"
            ))
        })?;
    resolve_api_key_in_section(
        &format!("{vendor_display} account {account:?}"),
        &format!("[[{section}.accounts]]"),
        entry.api_key_env.as_deref().unwrap_or(""),
        entry.api_key.as_deref(),
    )
}

pub(super) fn resolve_api_key_in_section(
    vendor_label: &str,
    section: &str,
    env_var_name: &str,
    inline: Option<&str>,
) -> crate::error::Result<String> {
    if let Some(key) = optional_api_key(env_var_name, inline) {
        return Ok(key);
    }
    let valid_env_name = is_valid_env_var_name(env_var_name);
    let advice = if valid_env_name {
        "set an API key in a valid environment variable or set `api_key`"
    } else {
        "fix the invalid `api_key_env` with a valid environment variable name or set `api_key`"
    };
    Err(crate::error::AppError::Unconfigured(format!(
        "{vendor_label}: no API key. Either {advice} under {section} in {}.",
        config_path_hint()
    )))
}

pub(super) fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Config {
    /// Load from `~/.config/ai-usagebar-omarchy/config.toml`. When the file doesn't
    /// exist yet, first drops the fully annotated example template there
    /// (best-effort — an unwritable config dir keeps the historical
    /// defaults-only behaviour) so a fresh install has a file-shaped place
    /// to fill in. Errors only on actual parse failures.
    pub fn load() -> Result<Self> {
        Self::load_with_note().map(|(config, _)| config)
    }

    /// [`Self::load`] plus the first-run seed note: `Some(path)` when this
    /// call freshly dropped the example template (the file did not exist).
    /// Terminal surfaces use it to tell the user where to configure; every
    /// other caller keeps [`Self::load`].
    pub fn load_with_note() -> Result<(Self, Option<PathBuf>)> {
        // Best-effort, idempotent: hoists a pre-rename `~/.config/ai-usagebar`
        // (and cache dir) out of the upstream project's reach on the first
        // run after upgrading to the fork. No-op for fresh and already-
        // migrated installs.
        migrate_legacy_layout();
        Self::load_with_note_at(resolved_path().as_deref())
    }

    /// Seed + parse against an explicit config path — the hermetic seam the
    /// `load_with_note` tests run through instead of the real `$HOME`.
    pub(super) fn load_with_note_at(config_path: Option<&Path>) -> Result<(Self, Option<PathBuf>)> {
        let Some(path) = config_path else {
            return Ok((Self::default(), None));
        };
        let note = match seed_example_config(path) {
            Ok(true) => {
                crate::diag::event(
                    "config/seed",
                    &format!("template written to {}", path.display()),
                );
                Some(path.to_path_buf())
            }
            Err(e) => {
                crate::diag::event("config/seed", &format!("failed: {}", e.user_message()));
                None
            }
            _ => None,
        };
        Ok((Self::load_from(path)?, note))
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                let mut config: Self = toml::from_str(&s)?;
                // `~` is shell syntax, not path syntax: `PathBuf` keeps it
                // literally, so a documented `credentials_path = "~/..."`
                // silently pointed at a directory named `~`.
                config.expand_paths();
                config.derive_account_labels();
                config.validate()?;
                #[cfg(unix)]
                config.protect_inline_api_keys(path)?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AppError::io_at(path, e)),
        }
    }

    fn expand_paths(&mut self) {
        expand_tilde_opt(&mut self.context.projects_path);
        expand_tilde_opt(&mut self.anthropic.credentials_path);
        expand_tilde_opt(&mut self.anthropic.accounts_dir);
        expand_tilde_opt(&mut self.anthropic.desktop_profiles_dir);
        expand_tilde_opt(&mut self.openai.codex_auth_path);
        expand_tilde_opt(&mut self.cursor.db_path);
        expand_tilde_opt(&mut self.cursor.agent_auth_path);
        expand_tilde_opt(&mut self.kiro.db_path);
        expand_tilde_opt(&mut self.kimi.credentials_path);
        self.supergrok.grok_binary = expand_tilde(&self.supergrok.grok_binary);
        expand_tilde_opt(&mut self.supergrok.auth_path);
        expand_tilde_opt(&mut self.supergrok.config_path);
        for account in &mut self.anthropic.accounts {
            account.credentials_path = expand_tilde(&account.credentials_path);
        }
    }

    /// Validate cross-entry constraints that serde cannot express. Account
    /// labels are both CLI selectors and TUI tab identities, so duplicates
    /// would make either destination ambiguous.
    pub fn validate(&self) -> Result<()> {
        if self.context.context_window_tokens == Some(0) {
            return Err(AppError::Other(
                "[context] context_window_tokens must be greater than zero".into(),
            ));
        }
        for (model, tokens) in &self.context.model_context_window_tokens {
            if model.trim().is_empty() {
                return Err(AppError::Other(
                    "[context] model_context_window_tokens keys must not be empty".into(),
                ));
            }
            if *tokens == 0 {
                return Err(AppError::Other(format!(
                    "[context] model_context_window_tokens entry {model:?} must be greater than zero"
                )));
            }
        }
        if let Some(limit) = self.anthropic_api.monthly_limit
            && (!limit.is_finite() || limit <= 0.0)
        {
            return Err(AppError::Other(
                "[anthropic_api] monthly_limit must be finite and greater than zero; \
                 remove it to show spend without a limit"
                    .into(),
            ));
        }
        if crate::kimi::oauth::Region::parse(&self.kimi.region).is_none()
            && !self.kimi.region.eq_ignore_ascii_case("auto")
        {
            return Err(AppError::Other(format!(
                "[kimi] region must be \"auto\", \"cn\", or \"global\", got {:?}",
                self.kimi.region
            )));
        }
        if !self.minimax.region.eq_ignore_ascii_case("global")
            && !self.minimax.region.eq_ignore_ascii_case("cn")
        {
            return Err(AppError::Other(format!(
                "[minimax] region must be \"global\" or \"cn\", got {:?}",
                self.minimax.region
            )));
        }
        if self.supergrok.grok_binary.as_os_str().is_empty() {
            return Err(AppError::Other(
                "[supergrok] grok_binary must not be empty".into(),
            ));
        }
        let mut labels = HashSet::new();
        for account in &self.anthropic.accounts {
            validate_account_label(&account.label)?;
            if !labels.insert(&account.label) {
                return Err(AppError::Credentials(format!(
                    "duplicate anthropic account label {:?}",
                    account.label
                )));
            }
        }
        // Key-account vendors carry no label constraints (labels are
        // positional, derived at load) and no key-presence requirement at
        // load — a keyless account simply reads as the calm "not configured
        // yet" state at fetch time. Spend limits are the one hard rule:
        // they feed arithmetic that NaN/zero would poison.
        for account in &self.anthropic_api.accounts {
            if let Some(limit) = account.monthly_limit
                && (!limit.is_finite() || limit <= 0.0)
            {
                return Err(AppError::Other(format!(
                    "[anthropic_api] account {} monthly_limit must be finite and greater \
                     than zero; remove it to show spend without a limit",
                    account.label
                )));
            }
        }
        if let Some(limit) = self.openai_api.monthly_limit
            && (!limit.is_finite() || limit <= 0.0)
        {
            return Err(AppError::Other(
                "[openai_api] monthly_limit must be finite and greater than zero; \
                 remove it to show spend without a limit"
                    .into(),
            ));
        }
        for account in &self.openai_api.accounts {
            if let Some(limit) = account.monthly_limit
                && (!limit.is_finite() || limit <= 0.0)
            {
                return Err(AppError::Other(format!(
                    "[openai_api] account {} monthly_limit must be finite and greater \
                     than zero; remove it to show spend without a limit",
                    account.label
                )));
            }
        }
        Ok(())
    }
}

/// Expand a leading `~` (or `~/`) against the user's home directory. Anything
/// else — including `~user` — is left untouched.
fn expand_tilde(p: &std::path::Path) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p.to_path_buf();
    };
    let rest = if s == "~" {
        ""
    } else if let Some(r) = s.strip_prefix("~/") {
        r
    } else {
        return p.to_path_buf();
    };
    match crate::cache::home_dir() {
        Ok(home) if rest.is_empty() => home,
        Ok(home) => home.join(rest),
        Err(_) => p.to_path_buf(),
    }
}

fn expand_tilde_opt(p: &mut Option<PathBuf>) {
    if let Some(inner) = p.as_ref() {
        *p = Some(expand_tilde(inner));
    }
}
