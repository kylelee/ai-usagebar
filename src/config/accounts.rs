use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    AnthropicConfig, Config, KimiConfig, OpenAiConfig, OpenRouterConfig, ResolvedZaiAccount,
    ZaiAccount, ZaiAccountType, ZaiConfig, ZaiSite, resolve_api_key, resolve_api_key_in_section,
    resolve_key_account,
};
use crate::anthropic::creds::CredsTarget;
use crate::cache::Cache;
use crate::error::{AppError, Result};

/// One extra Anthropic account beyond the default (issue #14). The default
/// account stays the singular `[anthropic] credentials_path`; each entry here
/// is an additional subscription selected on the CLI with `--account <label>`.
///
/// ```toml
/// [[anthropic.accounts]]
/// label = "work"
/// credentials_path = "~/.config/ai-usagebar-omarchy/accounts/work/.credentials.json"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AnthropicAccount {
    /// Stable name used on the CLI (`--account <label>`) and as the cache
    /// subdir (`~/.cache/ai-usagebar-omarchy/anthropic/<label>`).
    pub label: String,
    /// OAuth credentials file for this account (same JSON shape Claude Code
    /// writes). Token refreshes are written back here, so each account keeps
    /// itself alive independently.
    pub credentials_path: PathBuf,
}

impl AnthropicAccount {
    /// The `CLAUDE_CONFIG_DIR` this account occupies — the credential file's
    /// own directory. Claude Code hashes exactly this path for the account's
    /// Keychain item, so it is also the account's identity for
    /// [`crate::anthropic::keychain`].
    pub fn config_dir(&self) -> PathBuf {
        self.credentials_path
            .parent()
            .map_or_else(|| self.credentials_path.clone(), Path::to_path_buf)
    }
}

impl AnthropicConfig {
    /// Every extra account: the explicit `[[anthropic.accounts]]` entries plus
    /// any auto-discovered under [`accounts_dir`](AnthropicConfig::accounts_dir).
    /// Explicit entries take precedence on a label clash. This is what tabs and
    /// `--account` enumerate, so a discovered account behaves exactly like a
    /// hand-written one (own cache subdir, independent refresh).
    pub fn all_accounts(&self) -> Vec<AnthropicAccount> {
        let mut out = self.accounts.clone();
        if let Some(dir) = &self.accounts_dir {
            for acct in discover_accounts(dir) {
                if !out.iter().any(|a| a.label == acct.label) {
                    out.push(acct);
                }
            }
        }
        out
    }

    /// Find an extra account by label (explicit or discovered), or error listing
    /// the known labels so a typo fails loudly instead of silently hitting the
    /// default. Returns an owned account because discovered entries are
    /// synthesized, not stored.
    pub fn account(&self, label: &str) -> Result<AnthropicAccount> {
        validate_account_label(label)?;
        let all = self.all_accounts();
        all.iter().find(|a| a.label == label).cloned().ok_or_else(|| {
            let known: Vec<&str> = all.iter().map(|a| a.label.as_str()).collect();
            AppError::Credentials(format!(
                "anthropic account {label:?} not found in [[anthropic.accounts]] or accounts_dir; \
                 known labels: {known:?}"
            ))
        })
    }

    /// Resolve a named account to the credentials target + isolated cache it
    /// fetches through: [`CredsTarget::Named`], which on macOS prefers the
    /// Keychain item scoped to the file's own directory (that is where
    /// `CLAUDE_CONFIG_DIR=<dir> claude` actually writes) and falls back to
    /// the file elsewhere — never a *different* account's item, since the
    /// hash is per-directory, so issue #15's cross-account concern doesn't
    /// apply. Plus an `anthropic/<label>` cache subdir. Shared by the widget
    /// (`--account`) and the TUI's per-account tab (#14, #17) so both resolve
    /// accounts identically; the widget layers its `--cache-dir` override on
    /// top of the cache returned here.
    pub fn account_target(&self, label: &str) -> Result<(CredsTarget, Cache)> {
        let active = crate::anthropic::cli_account::home_claude_json()
            .ok()
            .and_then(|path| {
                crate::anthropic::cli_account::resolve_active_label(&path, &self.all_accounts())
            });
        self.account_target_with(label, active.as_deref())
    }

    /// The pure half of [`account_target`](AnthropicConfig::account_target),
    /// with "which account the `claude` CLI is signed into" injected — the same
    /// shape as `Cli::resolve_vendor_with`.
    ///
    /// When `label` *is* the live CLI login, its credential has been moved into
    /// the default slot and removed from its named slot. Reading the default
    /// one keeps exactly one live lineage, so a refresh here cannot invalidate
    /// the credential `claude` is using (or the other way round). The cache directory
    /// is unchanged either way, so the tab keeps its identity and its cached
    /// usage across a switch.
    pub fn account_target_with(
        &self,
        label: &str,
        cli_active: Option<&str>,
    ) -> Result<(CredsTarget, Cache)> {
        let account = self.account(label)?;
        let cache = Cache::for_vendor_account("anthropic", label)?;
        if cli_active == Some(label) {
            return Ok((
                CredsTarget::Default(crate::anthropic::creds::default_path()?),
                cache,
            ));
        }
        Ok((
            CredsTarget::Named {
                config_dir: account.config_dir(),
                path: account.credentials_path,
            },
            cache,
        ))
    }
}

/// The label doubles as a cache subdirectory name
/// (`~/.cache/ai-usagebar-omarchy/anthropic/<label>/`), which nests inside the default
/// account's cache dir — so path separators, control characters, or reserved
/// cache sidecar names would escape, spoof terminal output, or collide with the
/// cache layout (`usage.json`, `.stale`, …).
pub fn validate_account_label(label: &str) -> Result<()> {
    validate_account_label_for("anthropic", label)
}

pub(crate) fn validate_account_label_for(vendor: &str, label: &str) -> Result<()> {
    const RESERVED: [&str; 4] = ["usage.json", ".stale", ".last_error", ".fetch.lock"];
    let bad = label.is_empty()
        || label == "."
        || label == ".."
        || label.contains(['/', '\\'])
        || label.contains(':')
        || label.chars().any(char::is_control)
        || RESERVED.contains(&label);
    if bad {
        return Err(AppError::Credentials(format!(
            "invalid {vendor} account label {label:?}: must be a non-empty name \
             without path separators, drive prefixes, control characters, or reserved cache names"
        )));
    }
    Ok(())
}

/// Discover accounts under `accounts_dir` in the `CLAUDE_CONFIG_DIR` layout:
/// each immediate subdirectory becomes an account labeled by the subdirectory
/// name. Best-effort: an unreadable directory or unusable label is skipped
/// silently rather than failing the whole config — discovery is convenience,
/// while an explicit `[[anthropic.accounts]]` entry stays authoritative. The
/// fetch path resolves credentials from either `.credentials.json` or the macOS
/// Keychain. Sorted by label so the tab order is stable across runs.
fn discover_accounts(accounts_dir: &std::path::Path) -> Vec<AnthropicAccount> {
    let Ok(entries) = std::fs::read_dir(accounts_dir) else {
        return Vec::new();
    };
    let mut found: Vec<AnthropicAccount> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let label = path.file_name()?.to_str()?.to_string();
            validate_account_label(&label).ok()?;
            Some(AnthropicAccount {
                label,
                credentials_path: path.join(".credentials.json"),
            })
        })
        .collect();
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

/// Render a path with `$HOME` collapsed back to `~`, matching the style the docs
/// and existing `[[anthropic.accounts]]` entries use. Pure so it's testable;
/// paths outside home are returned verbatim.
pub fn tildify(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|rest| {
            let rendered = rest.display().to_string();
            // Config paths use the same portable `~/...` spelling on every
            // platform. A Windows `~\...` would not be expanded by the loader.
            #[cfg(windows)]
            let rendered = rendered.replace('\\', "/");
            format!("~/{rendered}")
        })
        .unwrap_or_else(|_| path.display().to_string())
}

/// Where a newly-registered account's credentials file lives by default: next
/// to `config.toml`, under `accounts/<label>/.credentials.json`. Returns the
/// absolute path (for `mkdir`) — tilde-render it with [`tildify`] for display
/// and for the value written into config.
pub fn default_account_credentials_path(config_path: &Path, label: &str) -> PathBuf {
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    base.join("accounts").join(label).join(".credentials.json")
}

/// Append a `[[anthropic.accounts]]` entry to a parsed config document, in
/// place. Pure over a `toml_edit` document so the validation, duplicate check,
/// and formatting are testable without disk. Preserves the rest of the file
/// (comments, key order, other sections) — only the new array-of-tables entry
/// is added. Errors on an invalid label or a label that already exists.
pub fn add_anthropic_account_to_doc(
    doc: &mut toml_edit::DocumentMut,
    label: &str,
    credentials_path: &str,
) -> Result<()> {
    use toml_edit::{Item, Table, value};

    validate_account_label(label)?;

    let anthropic = doc
        .entry("anthropic")
        .or_insert_with(|| Item::Table(Table::new()));
    let anthropic = anthropic
        .as_table_mut()
        .ok_or_else(|| AppError::Other("[anthropic] in config.toml is not a table".into()))?;

    let accounts = anthropic
        .entry("accounts")
        .or_insert_with(|| Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let accounts = accounts.as_array_of_tables_mut().ok_or_else(|| {
        AppError::Other("[[anthropic.accounts]] in config.toml is not an array of tables".into())
    })?;

    let exists = accounts
        .iter()
        .any(|t| t.get("label").and_then(Item::as_str) == Some(label));
    if exists {
        return Err(AppError::Credentials(format!(
            "anthropic account {label:?} already exists in config.toml"
        )));
    }

    let mut table = Table::new();
    table["label"] = value(label);
    table["credentials_path"] = value(credentials_path);
    accounts.push(table);
    Ok(())
}

/// One extra Codex login.
///
/// ```toml
/// [[openai.accounts]]
/// label = "work"
/// codex_auth_path = "~/.config/ai-usagebar-omarchy/accounts/work-codex/auth.json"
/// ```
///
/// A second login is made with `CODEX_HOME=~/.codex-work codex login`; point
/// `codex_auth_path` at the `auth.json` it writes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OpenAiAccount {
    /// Stable name used on the CLI (`--account <label>`) and as the cache
    /// subdir (`~/.cache/ai-usagebar-omarchy/openai/<label>`).
    pub label: String,
    /// Codex OAuth file for this account. Refreshed tokens are written back
    /// here, so each account keeps itself alive independently.
    pub codex_auth_path: PathBuf,
}

impl OpenAiConfig {
    /// The auth file for `label`, or the singular/default one when `label` is
    /// `None`. An unknown label is an error rather than a silent fall back to
    /// the default account, which would report the wrong login's usage.
    pub fn resolve_auth_path(&self, label: Option<&str>) -> Result<PathBuf> {
        let Some(label) = label else {
            return match &self.codex_auth_path {
                Some(path) => Ok(path.clone()),
                None => crate::openai::creds::default_path(),
            };
        };
        self.accounts
            .iter()
            .find(|account| account.label == label)
            .map(|account| account.codex_auth_path.clone())
            .ok_or_else(|| {
                AppError::Credentials(format!(
                    "no OpenAI account named {label:?}. Add it under \
                     [[openai.accounts]], or drop --account to use the default login."
                ))
            })
    }
}

impl ZaiConfig {
    /// Find a named account or fail loudly instead of falling back to the
    /// default key (which would show the wrong account's usage).
    pub fn account(&self, label: &str) -> Result<&ZaiAccount> {
        validate_account_label_for("zai", label)?;
        self.accounts
            .iter()
            .find(|account| account.label == label)
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .accounts
                    .iter()
                    .map(|account| account.label.as_str())
                    .collect();
                AppError::Credentials(format!(
                    "zai account {label:?} not found in [[zai.accounts]]; known labels: {known:?}"
                ))
            })
    }

    /// Resolve the default key or one named account into a validated fetch
    /// plan. Configured values are never included in an error message.
    ///
    /// A half-configured team entry (missing an id) fails loudly here rather
    /// than silently degrading to a personal query — the personal query
    /// would answer "not subscribed" for a team key and look like an error.
    pub fn resolve_account(&self, label: Option<&str>) -> Result<ResolvedZaiAccount> {
        let (api_key, account_type, organization_id, project_id, site, plan_tier) = match label {
            None => (
                resolve_api_key("Zai", &self.api_key_env, self.api_key.as_deref())?,
                self.account_type,
                self.organization_id.clone(),
                self.project_id.clone(),
                self.site,
                self.plan_tier.clone(),
            ),
            Some(label) => {
                let account = self.account(label)?;
                (
                    resolve_api_key_in_section(
                        &format!("Zai account {label:?}"),
                        "[[zai.accounts]]",
                        account.api_key_env.as_deref().unwrap_or(""),
                        account.api_key.as_deref(),
                    )?,
                    account.account_type,
                    account.organization_id.clone(),
                    account.project_id.clone(),
                    account.site,
                    account.plan_tier.clone(),
                )
            }
        };
        if account_type == ZaiAccountType::Team {
            let (organization_id, project_id) = match (&organization_id, &project_id) {
                (Some(org), Some(project)) if !org.is_empty() && !project.is_empty() => {
                    (organization_id, project_id)
                }
                _ => {
                    return Err(AppError::Credentials(
                        "zai: a team account needs both `organization_id` and `project_id` \
                         (bigmodel.cn console → F12 → Application → Local Storage → \
                         `Bigmodel-Organization` / `Bigmodel-Project`)"
                            .into(),
                    ));
                }
            };
            return Ok(ResolvedZaiAccount {
                api_key,
                account_type,
                site: site.unwrap_or_else(|| ZaiSite::default_for(account_type)),
                organization_id,
                project_id,
                plan_tier,
            });
        }
        Ok(ResolvedZaiAccount {
            api_key,
            account_type,
            site: site.unwrap_or_else(|| ZaiSite::default_for(account_type)),
            organization_id,
            project_id,
            plan_tier,
        })
    }
}

impl OpenRouterConfig {
    /// Find a named account or fail loudly instead of falling back to the
    /// default key (which would show the wrong account's usage).
    pub fn account(&self, label: &str) -> Result<&OpenRouterAccount> {
        validate_account_label_for("openrouter", label)?;
        self.accounts
            .iter()
            .find(|account| account.label == label)
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .accounts
                    .iter()
                    .map(|account| account.label.as_str())
                    .collect();
                AppError::Credentials(format!(
                    "openrouter account {label:?} not found in [[openrouter.accounts]]; \
                     known labels: {known:?}"
                ))
            })
    }

    /// Resolve either the backward-compatible default key or one named
    /// account. Configured values are never included in an error message.
    pub fn resolve_api_key(&self, label: Option<&str>) -> Result<String> {
        match label {
            None => resolve_api_key("OpenRouter", &self.api_key_env, self.api_key.as_deref()),
            Some(label) => {
                self.account(label)?;
                resolve_key_account("OpenRouter", "openrouter", &self.accounts, label)
            }
        }
    }
}

/// One extra API-key account for every key-based vendor
/// (`[[kimi.accounts]]`, `[[openrouter.accounts]]`, `[[deepseek.accounts]]`,
/// …). The runtime name is DERIVED at load: the entry's 1-based position in
/// its array ("1", "2", … — the settings panel's top-to-bottom order); the
/// config file carries no labels. `monthly_limit` exists only for the
/// spend-monitoring vendors (`anthropic_api`, `openai_api`) — each account
/// is its own organization with its own budget; the other vendors never
/// write it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct KeyAccount {
    #[serde(skip)]
    pub label: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub monthly_limit: Option<f64>,
}

/// One named Kimi account (`[[kimi.accounts]]`). Each entry reports its own
/// weekly + 5h subscription quota with an isolated cache.
pub type KimiAccount = KeyAccount;

/// One named OpenRouter account. The default account continues to use the
/// singular `api_key_env` / `api_key` fields under `[openrouter]`.
pub type OpenRouterAccount = KeyAccount;

impl KimiConfig {
    /// Find a named account or fail loudly instead of falling back to the
    /// default login (which would show the wrong subscription's usage).
    pub fn account(&self, label: &str) -> Result<&KimiAccount> {
        validate_account_label_for("kimi", label)?;
        self.accounts
            .iter()
            .find(|account| account.label == label)
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .accounts
                    .iter()
                    .map(|account| account.label.as_str())
                    .collect();
                AppError::Credentials(format!(
                    "kimi account {label:?} not found in [[kimi.accounts]]; known labels: {known:?}"
                ))
            })
    }

    /// Resolve the API key of one named account. Accounts are key-based: the
    /// Kimi Code CLI login is a single account and cannot back a named entry.
    pub fn resolve_account_key(&self, label: &str) -> Result<String> {
        self.account(label)?;
        resolve_key_account("Kimi", "kimi", &self.accounts, label)
    }
}

impl Config {
    /// Name Z.AI and every key-based vendor's accounts by POSITION ("1",
    /// "2", … in config = settings-panel order). The config files carry no
    /// labels; these derived names are the runtime identity for `--account`,
    /// report ids, cache subdirectories, and settings addressing. Anthropic
    /// and OpenAI (Codex) accounts keep their explicit labels — theirs come
    /// from external login directories and carry meaning a position cannot.
    pub(crate) fn derive_account_labels(&mut self) {
        for (index, account) in self.zai.accounts.iter_mut().enumerate() {
            account.label = (index + 1).to_string();
        }
        for accounts in self.key_account_sections_mut() {
            for (index, account) in accounts.iter_mut().enumerate() {
                account.label = (index + 1).to_string();
            }
        }
    }
}
