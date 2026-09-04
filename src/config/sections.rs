use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::vendor::VendorId;

use super::{AnthropicAccount, KeyAccount, KimiAccount, OpenAiAccount, OpenRouterAccount};

/// A misspelled section name is silently ignored without this: `[openrouer]`
/// leaves OpenRouter on its defaults and the user sees the wrong vendor set
/// with no diagnostic. Denying unknown keys is deliberately applied at the
/// *section* level only — the set of sections is small and stable, whereas
/// denying unknown keys inside every section would hard-fail configs that
/// carry a field from a future or removed version.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: UiConfig,
    pub context: ContextConfig,
    pub anthropic: AnthropicConfig,
    pub anthropic_api: AnthropicApiConfig,
    pub openai_api: OpenaiApiConfig,
    pub openai: OpenAiConfig,
    pub zai: ZaiConfig,
    pub openrouter: OpenRouterConfig,
    pub deepseek: DeepseekConfig,
    pub kimi: KimiConfig,
    pub kilo: KiloConfig,
    pub novita: NovitaConfig,
    pub moonshot: MoonshotConfig,
    pub grok: GrokConfig,
    pub supergrok: SuperGrokConfig,
    pub antigravity: AntigravityConfig,
    pub cursor: CursorConfig,
    pub minimax: MinimaxConfig,
    pub kiro: KiroConfig,
    #[serde(rename = "opencode-go")]
    pub opencode_go: OpenCodeGoConfig,
}

/// UI / dispatch preferences. Currently just `primary` — which vendor the
/// widget shows when `--vendor` is omitted, and which TUI tab is selected
/// at startup.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct UiConfig {
    /// `None` → fall back to anthropic for backward compatibility.
    pub primary: Option<VendorId>,
    /// Which vendors the Overview shows (the TUI's first tab and the macOS
    /// menu-bar's top section), in this order. `None` → every enabled vendor,
    /// in the canonical order.
    pub overview_vendors: Option<Vec<VendorId>>,
    /// Layout style for vendor navigation in the TUI: sidebar | navbar | none.
    pub vendor_box: Option<VendorBoxStyle>,
}

impl UiConfig {
    pub fn vendor_box(&self) -> VendorBoxStyle {
        self.vendor_box.unwrap_or_default()
    }
}

/// Presentation style of the TUI vendor navigation box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorBoxStyle {
    /// Vertical sidebar box on wide terminals; falls back to top navbar on narrow terminals.
    #[default]
    Sidebar,
    /// Horizontal navbar strip above the dashboard detail panel.
    Navbar,
    /// Completely hide vendor navigation (dashboards expand to fill full width).
    None,
}

/// Where the context view docks in the dashboard body. `v` cycles it while the
/// overlay is open; the config value is what it opens with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextLayout {
    /// Takes the whole body, the way a vendor panel does.
    #[default]
    Full,
    /// Beside the dashboard.
    Split,
    /// Below the dashboard.
    Bottom,
}

impl ContextLayout {
    pub fn next(self) -> Self {
        match self {
            ContextLayout::Full => ContextLayout::Split,
            ContextLayout::Split => ContextLayout::Bottom,
            ContextLayout::Bottom => ContextLayout::Full,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContextLayout::Full => "full",
            ContextLayout::Split => "split",
            ContextLayout::Bottom => "bottom",
        }
    }
}

/// Optional local Claude Code context-window monitor. This is deliberately
/// separate from vendors: sessions are discovered from local transcripts and
/// change while the TUI is running, whereas vendor tabs are config-declared
/// account identities.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Keep the filesystem scanner completely dormant unless explicitly
    /// enabled. The `c` key and its footer hint are hidden while disabled.
    pub enabled: bool,
    /// Override Claude Code's normal `~/.claude/projects` transcript root.
    pub projects_path: Option<PathBuf>,
    /// Optional fallback denominator. When absent, sessions without an exact
    /// model override show their input-token count without inventing a %.
    pub context_window_tokens: Option<u64>,
    /// Exact Claude model id -> context-window size. This takes precedence
    /// over `context_window_tokens`, which keeps mixed 200K/1M histories safe.
    pub model_context_window_tokens: BTreeMap<String, u64>,
    /// Where the view opens: full | split | bottom.
    pub layout: ContextLayout,
}

impl ContextConfig {
    pub fn window_tokens_for(&self, model: Option<&str>) -> Option<u64> {
        model
            .and_then(|model| self.model_context_window_tokens.get(model).copied())
            .filter(|tokens| *tokens > 0)
            .or_else(|| self.context_window_tokens.filter(|tokens| *tokens > 0))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnthropicConfig {
    pub enabled: bool,
    /// Override the credentials file path (defaults to `~/.claude/.credentials.json`).
    /// This is the *default* account; extra subscriptions go in `accounts`.
    pub credentials_path: Option<PathBuf>,
    /// Extra Anthropic accounts beyond the default, each selected on the CLI
    /// with `--account <label>` (issue #14). Empty by default, so existing
    /// single-account configs are byte-for-byte unchanged.
    pub accounts: Vec<AnthropicAccount>,
    /// Directory to auto-discover extra accounts from, in Claude Code's own
    /// `CLAUDE_CONFIG_DIR` layout: each immediate subdirectory becomes an
    /// account labeled by the subdirectory name. The credentials may live in
    /// that directory's `.credentials.json` or in the macOS Keychain, so
    /// discovery intentionally does not probe for the credentials file.
    /// Merged with `accounts` (explicit wins on a label clash); each is
    /// refreshed independently.
    pub accounts_dir: Option<PathBuf>,
    /// Whether the default (unnamed) Claude account gets its own tab. Defaults
    /// to `true` for back-compat. Set `false` when every account is managed
    /// explicitly (via `accounts`/`accounts_dir`) so the ambient
    /// Keychain/`~/.claude` login doesn't add a redundant "Claude" tab. Ignored
    /// when there are no named accounts, so Anthropic never loses its only tab.
    pub show_default_account: bool,
    /// Where the Claude **Desktop app**'s saved account profiles live. Defaults
    /// to `~/.claude-acc/profiles`, the store claude-acc
    /// (<https://github.com/ohmaseclaro/claude-acc>) creates — `account switch`
    /// reads and writes that layout so the two tools stay interchangeable.
    /// Unrelated to `accounts_dir`, which is the `claude` CLI's own accounts.
    pub desktop_profiles_dir: Option<PathBuf>,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            credentials_path: None,
            accounts: Vec::new(),
            accounts_dir: None,
            show_default_account: true,
            desktop_profiles_dir: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenAiConfig {
    pub enabled: bool,
    /// Override the Codex auth file path (defaults to `~/.codex/auth.json`).
    pub codex_auth_path: Option<PathBuf>,
    /// Extra Codex logins, each its own `auth.json`. Same shape as
    /// [`AnthropicAccount`] and for the same reason: Codex is an OAuth vendor,
    /// so an account *is* a credential file, and `openai::creds::write_back`
    /// refreshes into whichever one it read.
    #[serde(default)]
    pub accounts: Vec<OpenAiAccount>,
    /// Reserved, and inert: names the env var an API-key-only path *would*
    /// read (admin key → `/v1/organization/costs`). Nothing consumes it —
    /// OpenAI usage comes solely from Codex OAuth. Kept because that path is
    /// still intended, not for back-compat: `[openai]` doesn't deny unknown
    /// fields, so an existing `admin_key_env` would load either way. See
    /// `config.example.toml`, which ships it commented out so nobody sets it
    /// expecting an effect.
    pub admin_key_env: String,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            codex_auth_path: None,
            accounts: Vec::new(),
            admin_key_env: "OPENAI_ADMIN_KEY".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenCodeGoConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for OpenCodeGoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_env: "OPENCODE_GO_API_KEY".to_string(),
            api_key: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ZaiConfig {
    pub enabled: bool,
    /// Env var name to read the key from (env wins over `api_key`).
    pub api_key_env: String,
    /// Inline key (fallback when the env var is unset). Chmod 600 your
    /// config file if you put a real key here.
    pub api_key: Option<String>,
    /// Optional plan tier label (lite/pro/max) — display-only.
    pub plan_tier: Option<String>,
    /// How the default key is billed — see [`ZaiAccountType`]. Lets a single
    /// team key be configured without an accounts array.
    pub account_type: ZaiAccountType,
    /// Team plan organization/project ids (required when
    /// `account_type = "team"`). Read them from the bigmodel.cn console:
    /// F12 → Application → Local Storage → `Bigmodel-Organization` /
    /// `Bigmodel-Project`.
    pub organization_id: Option<String>,
    pub project_id: Option<String>,
    /// Which deployment the default key belongs to — see [`ZaiSite`].
    pub site: Option<ZaiSite>,
    /// Extra Z.AI / BigModel keys beyond the default one. Each entry gets a
    /// separate aggregate-view entry and cache directory.
    pub accounts: Vec<ZaiAccount>,
    /// Whether aggregate views include the default (unnamed) key when named
    /// accounts exist. Ignored when `accounts` is empty so Z.AI never loses
    /// its only tab.
    pub show_default_account: bool,
}

impl Default for ZaiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key_env: "ZAI_API_KEY".to_string(),
            api_key: None,
            plan_tier: None,
            account_type: ZaiAccountType::default(),
            organization_id: None,
            project_id: None,
            site: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

/// How one Z.AI / BigModel key is billed — decides which monitor endpoint
/// (if any) answers for it. Mirrors the three account types the old GNOME
/// panel extension shipped:
///
/// | type | query | shows |
/// | --- | --- | --- |
/// | personal (default) | quota (no params) | 5h + weekly windows |
/// | team | quota `?type=2` + org/project headers | same shape, team pools |
/// | usage | skips quota, 7-day usage stats only | prompts/tokens line |
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ZaiAccountType {
    /// Personal coding-plan subscription.
    #[default]
    Personal,
    /// Team / enterprise coding plan — same quota endpoint with `?type=2`
    /// plus `bigmodel-organization` / `bigmodel-project` headers, and both
    /// ids configured. Only exists on the CN site.
    Team,
    /// Pay-as-you-go key with no coding subscription — the quota endpoint
    /// answers "not subscribed" for it, so only the 7-day usage stats are
    /// fetched.
    Usage,
}

/// Which BigModel deployment a key belongs to. Both run the same monitor
/// gateway on the same paths; only the host differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ZaiSite {
    /// Z.ai international (`api.z.ai`) — the historical default of this tool.
    Global,
    /// BigModel China (`open.bigmodel.cn`). The team/enterprise plan and
    /// pay-as-you-go coding keys only exist here.
    Cn,
}

impl ZaiSite {
    /// The site a key of `account_type` lives on when `site` is not pinned.
    /// Personal keys default to the historical Z.ai host so existing configs
    /// never move; team and usage-only keys only exist on BigModel CN.
    pub fn default_for(account_type: ZaiAccountType) -> Self {
        match account_type {
            ZaiAccountType::Personal => Self::Global,
            ZaiAccountType::Team | ZaiAccountType::Usage => Self::Cn,
        }
    }
}

/// One extra Z.AI / BigModel account. The default key continues to use the
/// singular fields under `[zai]`; each entry here adds a key with its own
/// billing type, site, and cache directory. Entries carry no label — the
/// runtime name is the entry's 1-based position in the array.
///
/// ```toml
/// [[zai.accounts]]
/// api_key_env = "BIGMODEL_TEAM_KEY"
/// account_type = "team"
/// organization_id = "org-…"
/// project_id = "proj-…"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ZaiAccount {
    /// Runtime identity, DERIVED at load: the entry's 1-based position in
    /// `[[zai.accounts]]` ("1", "2", …) — the settings panel's top-to-bottom
    /// order. The config file no longer carries a `label`; a hand-written
    /// one is ignored (and dropped whenever the settings bridge touches the
    /// entry). Positional names keep `--account`, report ids, and cache
    /// subdirectories stable without any user configuration.
    #[serde(skip)]
    pub label: String,
    /// Optional environment variable containing this account's key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Inline fallback when the account environment variable is unset.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Optional display-only plan tier for this account.
    #[serde(default)]
    pub plan_tier: Option<String>,
    #[serde(default)]
    pub account_type: ZaiAccountType,
    #[serde(default)]
    pub organization_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub site: Option<ZaiSite>,
}

/// Everything a fetch needs for one Z.AI key, resolved and validated from
/// either the `[zai]` section or a `[[zai.accounts]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedZaiAccount {
    pub api_key: String,
    pub account_type: ZaiAccountType,
    pub site: ZaiSite,
    pub organization_id: Option<String>,
    pub project_id: Option<String>,
    pub plan_tier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenRouterConfig {
    pub enabled: bool,
    /// Extra OpenRouter accounts beyond the default key. Each account gets a
    /// separate aggregate-view entry and cache directory.
    pub accounts: Vec<OpenRouterAccount>,
    /// Whether aggregate views include the default (unnamed) key when named
    /// accounts exist. Ignored when `accounts` is empty so OpenRouter never
    /// loses its only tab.
    pub show_default_account: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            accounts: Vec::new(),
            show_default_account: true,
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct DeepseekConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for DeepseekConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            api_key: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct KimiConfig {
    pub enabled: bool,
    pub api_key_env: String,
    /// Optional: with no key set, the vendor falls back to the Kimi Code CLI's
    /// own OAuth login, which is what a subscriber already has locally.
    pub api_key: Option<String>,
    /// Override for kimi-code's credential file (default
    /// `~/.kimi-code/credentials/kimi-code.json`), mirroring `[cursor] db_path`
    /// and `[kiro] db_path`. Useful with a relocated `KIMI_CODE_HOME`.
    pub credentials_path: Option<PathBuf>,
    /// `"auto"` follows kimi-code's own install marker (`~/.kimi-code/region`);
    /// `"cn"` pins `api.kimi.com` / `auth.kimi.com`, `"global"` pins
    /// `api.kimi.ai` / `auth.kimi.ai`. A token minted by one deployment means
    /// nothing to the other, so this picks the instance, not a currency.
    pub region: String,
    /// Extra Kimi For Coding subscriptions beyond the default one. Accounts
    /// are API-key based — the CLI OAuth login is a single account, so named
    /// entries must carry their own key (issue a key per subscription at
    /// kimi.com/code/console).
    pub accounts: Vec<KimiAccount>,
    /// Whether aggregate views include the default login/key when named
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for KimiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_env: "KIMI_API_KEY".to_string(),
            api_key: None,
            credentials_path: None,
            region: "auto".to_string(),
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct KiloConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Optional Kilo organization id — scopes the balance to a team via the
    /// `x-kilocode-organizationid` header. Omit for the personal balance.
    pub organization_id: Option<String>,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for KiloConfig {
    fn default() -> Self {
        // Opt-in like DeepSeek: requires an explicit API key, so it defaults to
        // disabled and never affects existing installs.
        Self {
            enabled: false,
            api_key_env: "KILO_API_KEY".to_string(),
            api_key: None,
            organization_id: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct NovitaConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for NovitaConfig {
    fn default() -> Self {
        // Opt-in like DeepSeek/Kilo: needs an explicit API key.
        Self {
            enabled: false,
            api_key_env: "NOVITA_API_KEY".to_string(),
            api_key: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MinimaxConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// `"global"` → api.minimax.io; `"cn"` → api.minimaxi.com. Unlike
    /// Moonshot's, this does not change the unit — MiniMax reports quota as a
    /// percentage either way. It picks the *instance*: a key issued for one
    /// host is rejected by the other (`status_code 2049`), so pointing this at
    /// the wrong region reads as an invalid key rather than an empty plan.
    pub region: String,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for MinimaxConfig {
    fn default() -> Self {
        // Opt-in like the other API-key vendors: needs an explicit key.
        Self {
            enabled: false,
            api_key_env: "MINIMAX_API_KEY".to_string(),
            api_key: None,
            region: "global".to_string(),
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MoonshotConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// `"global"` → api.moonshot.ai (USD); `"cn"` → api.moonshot.cn (CNY).
    pub region: String,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for MoonshotConfig {
    fn default() -> Self {
        // Opt-in like DeepSeek/Kilo/Novita: needs an explicit API key.
        Self {
            enabled: false,
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            api_key: None,
            region: "global".to_string(),
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GrokConfig {
    pub enabled: bool,
    /// Env var for the xAI **Management** key (distinct from the inference key).
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Optional team id. When absent, it's auto-resolved from the management
    /// key via `/auth/management-keys/validation`.
    pub team_id: Option<String>,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for GrokConfig {
    fn default() -> Self {
        // Opt-in: needs a management key (and, for prepaid, a team).
        Self {
            enabled: false,
            api_key_env: "XAI_MANAGEMENT_KEY".to_string(),
            api_key: None,
            team_id: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

/// SuperGrok subscription auth — no API key. Asks the official Grok Build
/// CLI for billing through its `x.ai/billing` ACP extension, leaving every
/// credential, issuer, proxy, and token-rotation decision inside Grok Build.
///
/// Opt-in like Cursor/Kiro (`enabled` defaults to `false`): it requires a
/// separate official executable and signed-in session, so it stays off until
/// the user explicitly turns it on.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SuperGrokConfig {
    pub enabled: bool,
    /// Trusted official Grok Build executable. Defaults to its canonical
    /// `$GROK_HOME/bin/grok` (or `~/.grok/bin/grok`) installation path instead
    /// of searching PATH, where unrelated programs can share the name.
    pub grok_binary: PathBuf,
    /// Opaque auth/config files used only to fingerprint the active cache
    /// scope. Their contents are never parsed or copied to the cache.
    pub auth_path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
}

impl Default for SuperGrokConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            grok_binary: default_grok_binary(),
            auth_path: None,
            config_path: None,
        }
    }
}

pub(super) fn default_grok_binary() -> PathBuf {
    let executable = if cfg!(windows) { "grok.exe" } else { "grok" };
    let grok_home = std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::cache::home_dir().ok().map(|home| home.join(".grok")));
    grok_home
        .map(|home| home.join("bin").join(executable))
        .unwrap_or_else(|| PathBuf::from(executable))
}

/// Antigravity reads its quota from whichever local Antigravity product is
/// running, so it needs no credentials — only an on/off switch.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AntigravityConfig {
    pub enabled: bool,
}

/// Cursor reads its quota through a session token the Cursor IDE already
/// wrote to its local `state.vscdb` — no API key, but (unlike Antigravity)
/// there is a real on-disk path that can need overriding (e.g. a portable or
/// non-default Cursor install), mirroring `openai.codex_auth_path`.
///
/// Opt-in like DeepSeek/Kilo/etc (`enabled` defaults to `false`, matching
/// `bool::default()`): reads an undocumented endpoint via a session token
/// scraped from a local IDE file, so it stays off until the user explicitly
/// turns it on.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CursorConfig {
    pub enabled: bool,
    /// Override Cursor's local state database path (defaults to the
    /// platform-standard `.../User/globalStorage/state.vscdb` — see
    /// `cursor::db::default_db_path`).
    pub db_path: Option<PathBuf>,
    /// Override the headless `cursor-agent` CLI's own login file (defaults to
    /// `.../cursor/auth.json` — see `cursor::db::default_agent_auth_path`).
    /// Used as a fallback when `db_path` doesn't exist, so a text-only
    /// machine that never runs the desktop IDE still gets usage.
    pub agent_auth_path: Option<PathBuf>,
}

/// Kiro CLI reads its quota through the AWS SSO OIDC session kiro-cli already
/// wrote to its own local `data.sqlite3` — no API key, but (like Cursor) a
/// real on-disk path that can need overriding.
///
/// Opt-in like Cursor/DeepSeek/Kilo/etc (`enabled` defaults to `false`):
/// calls a reverse-engineered CodeWhisperer endpoint via a session token
/// scraped from a local CLI database, so it stays off until the user
/// explicitly turns it on.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct KiroConfig {
    pub enabled: bool,
    /// Override kiro-cli's local database path (defaults to the
    /// platform-standard `.../kiro-cli/data.sqlite3` — see
    /// `kiro::db::default_db_path`).
    pub db_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnthropicApiConfig {
    pub enabled: bool,
    /// Env var for the Console **Admin key** (`sk-ant-admin01-…`), distinct from
    /// an inference key and from the Claude Code OAuth login.
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Monthly USD spend limit, used only for the spend-vs-limit % display. The
    /// API exposes neither this limit nor the remaining prepaid balance.
    pub monthly_limit: Option<f64>,
    /// Extra keys beyond the default one (`[[vendor.accounts]]`): each entry
    /// gets its own tab, tile, and isolated cache, auto-named by its
    /// position ("1", "2", …). The default key keeps the singular fields.
    pub accounts: Vec<KeyAccount>,
    /// Whether aggregate views include the default key when numbered
    /// accounts exist. Ignored when `accounts` is empty.
    pub show_default_account: bool,
}

impl Default for AnthropicApiConfig {
    fn default() -> Self {
        // Opt-in: needs an explicit Admin key.
        Self {
            enabled: false,
            api_key_env: "ANTHROPIC_ADMIN_KEY".to_string(),
            api_key: None,
            monthly_limit: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}

/// OpenAI Admin API — trailing-30-day spend from the Costs API over a
/// platform **Admin key** (regular project `sk-` keys are rejected by the
/// organization endpoints). There is no balance endpoint: spend against an
/// optional monthly limit is the whole display, mirroring
/// [`AnthropicApiConfig`].
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenaiApiConfig {
    pub enabled: bool,
    /// Env var for the platform **Admin key** (organization settings).
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Monthly USD spend limit, used only for the spend-vs-limit % display.
    pub monthly_limit: Option<f64>,
    pub accounts: Vec<KeyAccount>,
    pub show_default_account: bool,
}

impl Default for OpenaiApiConfig {
    fn default() -> Self {
        // Opt-in: needs an explicit Admin key.
        Self {
            enabled: false,
            api_key_env: "OPENAI_ADMIN_KEY".to_string(),
            api_key: None,
            monthly_limit: None,
            accounts: Vec::new(),
            show_default_account: true,
        }
    }
}
