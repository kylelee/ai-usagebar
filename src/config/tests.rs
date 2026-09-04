use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn write_toml(s: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(s.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

/// The seed note is Some exactly when this call created the template:
/// Some on a fresh dir, None once the file exists (the template is never
/// rewritten).
#[test]
fn load_with_note_reports_the_seed_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    let (_, note) = Config::load_with_note_at(Some(&path)).unwrap();
    assert_eq!(note.as_deref(), Some(path.as_path()));

    let (_, second) = Config::load_with_note_at(Some(&path)).unwrap();
    assert_eq!(second, None);
}

/// No resolvable config path keeps the defaults-only behavior and has
/// nothing to note.
#[test]
fn load_with_note_at_none_yields_defaults_and_no_note() {
    let (config, note) = Config::load_with_note_at(None).unwrap();
    assert_eq!(note, None);
    assert_eq!(config.zai.accounts, Vec::new());
}

/// Z.AI and Kimi accounts are named by POSITION on load ("1", "2", … —
/// the settings panel's top-to-bottom order); the config file carries no
/// labels, and a legacy hand-written label is ignored. Anthropic and
/// OpenRouter labels are external identities and stay verbatim.
#[test]
fn zai_and_kimi_accounts_are_named_by_position() {
    let file = write_toml(
        r#"
        [[zai.accounts]]
        label = "work"
        api_key = "k1"

        [[zai.accounts]]
        api_key = "k2"

        [[kimi.accounts]]
        label = "legacy"
        api_key = "k3"
        "#,
    );
    let cfg = Config::load_from(file.path()).unwrap();
    let zai: Vec<&str> = cfg.zai.accounts.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(zai, vec!["1", "2"], "positions replace stored labels");
    let kimi: Vec<&str> = cfg.kimi.accounts.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(kimi, vec!["1"]);

    // The derived names address accounts everywhere the runtime does.
    assert_eq!(cfg.zai.account("2").unwrap().api_key.as_deref(), Some("k2"));
    assert!(
        cfg.zai.account("work").is_err(),
        "legacy labels no longer address"
    );
    assert!(
        cfg.zai.account("3").is_err(),
        "out-of-range position refused"
    );
}

/// A label-less TOML parse (no load_from normalization) leaves labels
/// empty — derive_account_labels is what names them, so direct parses
/// (settings snapshot) call it too.
#[test]
fn derive_account_labels_numbers_only_zai_and_kimi() {
    let mut cfg: Config = toml::from_str(
        r#"
        [[zai.accounts]]
        api_key = "k1"

        [[zai.accounts]]
        api_key = "k2"

        [[anthropic.accounts]]
        label = "work"
        credentials_path = "/tmp/c.json"
        "#,
    )
    .unwrap();
    assert!(cfg.zai.accounts.iter().all(|a| a.label.is_empty()));
    cfg.derive_account_labels();
    let zai: Vec<&str> = cfg.zai.accounts.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(zai, vec!["1", "2"]);
    // Anthropic labels are external identities (login directories) —
    // never renumbered.
    assert_eq!(cfg.anthropic.accounts[0].label, "work");
}

/// The back-compat guarantee #134 asks for: a config with no
/// `[[openai.accounts]]` resolves exactly what it resolved before, whether
/// it sets `codex_auth_path` or leaves it to the default.
#[test]
fn openai_without_accounts_resolves_the_singular_path() {
    let explicit = OpenAiConfig {
        codex_auth_path: Some(PathBuf::from("/tmp/codex/auth.json")),
        ..OpenAiConfig::default()
    };
    assert_eq!(
        explicit.resolve_auth_path(None).unwrap(),
        PathBuf::from("/tmp/codex/auth.json")
    );

    let bare = OpenAiConfig::default();
    assert_eq!(
        bare.resolve_auth_path(None).unwrap(),
        crate::openai::creds::default_path().unwrap(),
        "no codex_auth_path must still mean ~/.codex/auth.json"
    );
}

/// Each named account resolves its own file, and the default login is still
/// reachable alongside them.
#[test]
fn openai_named_accounts_resolve_their_own_auth_file() {
    let config: Config = toml::from_str(
        r#"
        [openai]
        codex_auth_path = "/tmp/personal/auth.json"
        [[openai.accounts]]
        label = "work"
        codex_auth_path = "/tmp/work/auth.json"
        "#,
    )
    .unwrap();

    assert_eq!(
        config.openai.resolve_auth_path(Some("work")).unwrap(),
        PathBuf::from("/tmp/work/auth.json")
    );
    assert_eq!(
        config.openai.resolve_auth_path(None).unwrap(),
        PathBuf::from("/tmp/personal/auth.json")
    );
}

/// An unknown label must fail rather than quietly fall back to the default
/// login — reporting the wrong subscription's usage is worse than an error.
#[test]
fn an_unknown_openai_account_is_an_error_not_a_fallback() {
    let config = OpenAiConfig {
        codex_auth_path: Some(PathBuf::from("/tmp/personal/auth.json")),
        accounts: vec![OpenAiAccount {
            label: "work".into(),
            codex_auth_path: PathBuf::from("/tmp/work/auth.json"),
        }],
        ..OpenAiConfig::default()
    };
    let err = config
        .resolve_auth_path(Some("nope"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("nope"), "{err}");
    assert!(err.contains("[[openai.accounts]]"), "{err}");
}

#[test]
fn defaults_enable_only_the_four_core_vendors() {
    let c = Config::default();
    assert!(c.is_enabled(VendorId::Anthropic));
    assert!(c.is_enabled(VendorId::Openai));
    assert!(c.is_enabled(VendorId::Zai));
    assert!(c.is_enabled(VendorId::Openrouter));
    for opt_in in [
        VendorId::AnthropicApi,
        VendorId::Deepseek,
        VendorId::Kimi,
        VendorId::Kilo,
        VendorId::Novita,
        VendorId::Moonshot,
        VendorId::Grok,
        VendorId::Supergrok,
        VendorId::Cursor,
        VendorId::Minimax,
        VendorId::Kiro,
    ] {
        assert!(!c.is_enabled(opt_in), "{opt_in:?}");
    }
    assert_eq!(c.enabled_vendors().len(), 4);
}

#[test]
fn new_provider_defaults_are_opt_in_and_use_exact_auth_contracts() {
    let config = Config::default();
    assert!(!config.is_enabled(VendorId::OpenCodeGo));
    assert_eq!(config.opencode_go.api_key_env, "OPENCODE_GO_API_KEY");
    assert!(config.opencode_go.api_key.is_none());
}

#[cfg(unix)]
#[test]
fn opencode_go_inline_key_is_protected_like_other_api_keys() {
    let mut config = Config::default();
    config.opencode_go.api_key = Some("<redacted>".to_string());
    assert!(config.has_inline_api_keys());
}

#[cfg(unix)]
#[test]
fn openrouter_named_inline_keys_receive_config_file_protection() {
    let mut config = Config::default();
    config.openrouter.accounts.push(OpenRouterAccount {
        label: "work".into(),
        api_key_env: None,
        api_key: Some("<redacted>".into()),
        monthly_limit: None,
    });
    assert!(config.has_inline_api_keys());
}

#[test]
fn missing_file_uses_defaults() {
    let path = std::path::Path::new("/tmp/does-not-exist-ai-usagebar-test");
    let c = Config::load_from(path).unwrap();
    assert!(c.is_enabled(VendorId::Anthropic));
}

#[test]
fn parses_full_config() {
    let f = write_toml(
        r#"
        [anthropic]
        enabled = true

        [openai]
        enabled = false
        admin_key_env = "MY_ADMIN_KEY"

        [zai]
        enabled = true
        api_key_env = "MY_ZAI"
        plan_tier = "pro"

        [openrouter]
        enabled = false
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.is_enabled(VendorId::Anthropic));
    assert!(!c.is_enabled(VendorId::Openai));
    assert!(c.is_enabled(VendorId::Zai));
    assert!(!c.is_enabled(VendorId::Openrouter));
    assert_eq!(c.openai.admin_key_env, "MY_ADMIN_KEY");
    assert_eq!(c.zai.api_key_env, "MY_ZAI");
    assert_eq!(c.zai.plan_tier.as_deref(), Some("pro"));
    assert!(c.openrouter.accounts.is_empty());
    assert!(c.openrouter.show_default_account);
}

#[test]
fn partial_config_falls_back_to_defaults() {
    let f = write_toml(
        r#"[openai]
enabled = false
"#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(!c.is_enabled(VendorId::Openai));
    // Other vendors keep their defaults.
    assert!(c.is_enabled(VendorId::Anthropic));
    assert_eq!(c.openai.admin_key_env, "OPENAI_ADMIN_KEY");
}

#[test]
fn malformed_toml_returns_error() {
    let f = write_toml("this is not = = valid");
    assert!(Config::load_from(f.path()).is_err());
}

#[cfg(unix)]
#[test]
fn load_from_tightens_world_readable_config_with_inline_api_key() {
    let file = write_toml("[zai]\napi_key = \"test-inline-key\"\n");
    std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

    Config::load_from(file.path()).unwrap();

    assert_eq!(
        std::fs::metadata(file.path()).unwrap().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn load_from_leaves_world_readable_config_without_inline_api_keys_unchanged() {
    let file = write_toml("[zai]\napi_key_env = \"TEST_ZAI_API_KEY\"\n");
    std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

    Config::load_from(file.path()).unwrap();

    assert_eq!(
        std::fs::metadata(file.path()).unwrap().mode() & 0o777,
        0o644
    );
}

#[cfg(unix)]
#[test]
fn inline_key_permission_decision_requires_tightening_for_group_or_other_bits() {
    assert_eq!(
        inline_key_permission_decision(0o600),
        InlineKeyPermissionDecision::Ok
    );
    assert_eq!(
        inline_key_permission_decision(0o640),
        InlineKeyPermissionDecision::Tighten
    );
    assert_eq!(
        inline_key_permission_decision(0o604),
        InlineKeyPermissionDecision::Tighten
    );
}

#[test]
fn anthropic_api_monthly_limit_must_be_positive_and_finite() {
    for value in ["0", "-1", "inf", "nan"] {
        let file = write_toml(&format!("[anthropic_api]\nmonthly_limit = {value}\n"));
        let error = Config::load_from(file.path()).unwrap_err().to_string();
        assert!(error.contains("monthly_limit"), "value {value}: {error}");
    }

    let file = write_toml("[anthropic_api]\nmonthly_limit = 1000\n");
    assert_eq!(
        Config::load_from(file.path())
            .unwrap()
            .anthropic_api
            .monthly_limit,
        Some(1000.0)
    );
}

#[test]
fn minimax_region_accepts_only_known_instances() {
    for region in ["global", "GLOBAL", "cn", "CN"] {
        let file = write_toml(&format!("[minimax]\nregion = {region:?}\n"));
        assert_eq!(
            Config::load_from(file.path()).unwrap().minimax.region,
            region
        );
    }

    for region in ["", "china", "us"] {
        let file = write_toml(&format!("[minimax]\nregion = {region:?}\n"));
        let error = Config::load_from(file.path()).unwrap_err().to_string();
        assert!(error.contains("[minimax] region"), "{error}");
    }
}

#[test]
fn kimi_region_accepts_auto_and_both_deployments() {
    for region in ["auto", "AUTO", "cn", "mainland-cn", "global"] {
        let file = write_toml(&format!("[kimi]\nregion = {region:?}\n"));
        assert_eq!(Config::load_from(file.path()).unwrap().kimi.region, region);
    }

    for region in ["", "us", "oversea"] {
        let file = write_toml(&format!("[kimi]\nregion = {region:?}\n"));
        let error = Config::load_from(file.path()).unwrap_err().to_string();
        assert!(error.contains("[kimi] region"), "{error}");
    }
}

#[test]
fn kimi_defaults_to_auto_region_and_no_credential_override() {
    let defaults = KimiConfig::default();
    assert_eq!(defaults.region, "auto");
    assert_eq!(defaults.credentials_path, None);
    assert!(!defaults.enabled);
}

#[test]
fn kimi_credentials_path_expands_a_tilde() {
    let file = write_toml("[kimi]\ncredentials_path = \"~/kimi/creds.json\"\n");
    let path = Config::load_from(file.path())
        .unwrap()
        .kimi
        .credentials_path
        .unwrap();
    assert!(!path.starts_with("~"), "{}", path.display());
    assert!(path.ends_with("kimi/creds.json"), "{}", path.display());
}

#[test]
fn optional_api_key_reports_absence_instead_of_failing() {
    assert_eq!(
        optional_api_key("KIMI_API_KEY_DEFINITELY_UNSET", Some("inline")),
        Some("inline".to_string())
    );
    assert_eq!(
        optional_api_key("KIMI_API_KEY_DEFINITELY_UNSET", None),
        None
    );
    assert_eq!(optional_api_key("KIMI_API_KEY_UNSET", Some("")), None);
    // An unusable `api_key_env` still lets an inline key through, exactly
    // as `resolve_api_key` does.
    assert_eq!(
        optional_api_key("9INVALID", Some("inline")),
        Some("inline".to_string())
    );
}

#[test]
fn context_monitor_is_opt_in_and_window_sizes_are_explicit() {
    let defaults = Config::default();
    assert!(!defaults.context.enabled);
    assert_eq!(
        defaults.context.window_tokens_for(Some("claude-test")),
        None
    );

    let file = write_toml(
        r#"
        [context]
        enabled = true
        context_window_tokens = 200000

        [context.model_context_window_tokens]
        claude-opus-1m = 1000000
        "claude exact id" = 300000
        "#,
    );
    let config = Config::load_from(file.path()).unwrap();
    assert!(config.context.enabled);
    assert_eq!(
        config.context.window_tokens_for(Some("claude-opus-1m")),
        Some(1_000_000)
    );
    assert_eq!(
        config.context.window_tokens_for(Some("claude exact id")),
        Some(300_000)
    );
    assert_eq!(
        config.context.window_tokens_for(Some("another-model")),
        Some(200_000)
    );
}

#[test]
fn context_layout_defaults_to_full_and_parses_each_variant() {
    assert_eq!(Config::default().context.layout, ContextLayout::Full);
    for (text, want) in [
        ("full", ContextLayout::Full),
        ("split", ContextLayout::Split),
        ("bottom", ContextLayout::Bottom),
    ] {
        let file = write_toml(&format!("[context]\nlayout = \"{text}\"\n"));
        assert_eq!(Config::load_from(file.path()).unwrap().context.layout, want);
    }
    let file = write_toml("[context]\nlayout = \"floating\"\n");
    assert!(
        Config::load_from(file.path()).is_err(),
        "an unknown layout must be rejected, not silently defaulted"
    );
}

#[test]
fn vendor_box_defaults_to_sidebar_and_parses_each_variant() {
    assert_eq!(Config::default().ui.vendor_box(), VendorBoxStyle::Sidebar);
    for (text, want) in [
        ("sidebar", VendorBoxStyle::Sidebar),
        ("navbar", VendorBoxStyle::Navbar),
        ("none", VendorBoxStyle::None),
    ] {
        let file = write_toml(&format!("[ui]\nvendor_box = \"{text}\"\n"));
        assert_eq!(
            Config::load_from(file.path()).unwrap().ui.vendor_box(),
            want
        );
    }
    let file = write_toml("[ui]\nvendor_box = \"floating\"\n");
    assert!(
        Config::load_from(file.path()).is_err(),
        "an unknown vendor_box style must be rejected, not silently defaulted"
    );
}

#[test]
fn context_window_sizes_must_be_nonzero_and_model_ids_nonempty() {
    for source in [
        "[context]\ncontext_window_tokens = 0\n",
        "[context.model_context_window_tokens]\nclaude = 0\n",
        "[context.model_context_window_tokens]\n\" \" = 200000\n",
    ] {
        let file = write_toml(source);
        let error = Config::load_from(file.path()).unwrap_err().to_string();
        assert!(error.contains("context"), "{error}");
    }
}

// serial guard for env-var manipulation tests so they don't race
fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
    M.lock().unwrap_or_else(|p| p.into_inner())
}

#[test]
fn resolve_api_key_prefers_env_over_inline() {
    let _g = env_guard();
    // Use a unique env var name so we don't clobber test parallelism.
    let var = "AI_USAGEBAR_TEST_ENV_WINS";
    // SAFETY: tests are single-threaded under env_guard.
    unsafe { std::env::set_var(var, "from-env") };
    let got = resolve_api_key("Zai", var, Some("from-inline")).unwrap();
    unsafe { std::env::remove_var(var) };
    assert_eq!(got, "from-env");
}

#[test]
fn resolve_api_key_falls_back_to_inline() {
    let _g = env_guard();
    let var = "AI_USAGEBAR_TEST_INLINE_FALLBACK";
    unsafe { std::env::remove_var(var) };
    let got = resolve_api_key("Zai", var, Some("inline-key")).unwrap();
    assert_eq!(got, "inline-key");
}

#[test]
fn resolve_api_key_errors_when_both_missing() {
    let _g = env_guard();
    let var = "AI_USAGEBAR_TEST_BOTH_MISSING";
    unsafe { std::env::remove_var(var) };
    let err = resolve_api_key("Zai", var, None).unwrap_err();
    match err {
        crate::error::AppError::Unconfigured(msg) => {
            assert!(
                msg.contains("api_key"),
                "error should suggest config field: {msg}"
            );
        }
        other => panic!("expected the Unconfigured remedy, got {other:?}"),
    }
}

#[test]
fn resolve_api_key_uses_exact_opencode_go_section_name() {
    let _g = env_guard();
    unsafe { std::env::remove_var("OPENCODE_GO_API_KEY") };
    let err = resolve_api_key("OpenCode Go", "OPENCODE_GO_API_KEY", None).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("[opencode-go]"),
        "wrong section hint: {message}"
    );
    assert!(
        !message.contains("[opencode go]"),
        "wrong section hint: {message}"
    );
}

#[test]
fn config_path_hint_ends_with_config_toml() {
    // Platform-resolved (Linux/macOS/Windows), but always ends in the
    // config filename — the trailing segment is what messages rely on.
    assert!(config_path_hint().ends_with("config.toml"));
}

#[test]
fn missing_api_key_is_unconfigured_and_keeps_todays_display_text() {
    let _g = env_guard();
    unsafe { std::env::remove_var("AI_USAGEBAR_TASK9_MISSING_KEY") };
    let err = resolve_api_key("Deepseek", "AI_USAGEBAR_TASK9_MISSING_KEY", None).unwrap_err();
    assert!(
        crate::report::is_unconfigured_app_error(&err),
        "no key is the calm state, not an alarm: {err}"
    );
    assert_eq!(
        err.to_string(),
        format!(
            "credentials error: Deepseek: no API key. Either set an API key in a valid environment variable or set `api_key` under [deepseek] in {}.",
            config_path_hint()
        )
    );
}

#[test]
fn resolve_api_key_treats_empty_env_as_unset() {
    let _g = env_guard();
    let var = "AI_USAGEBAR_TEST_EMPTY_ENV";
    unsafe { std::env::set_var(var, "") };
    let got = resolve_api_key("OpenRouter", var, Some("inline")).unwrap();
    unsafe { std::env::remove_var(var) };
    assert_eq!(got, "inline");
}

#[test]
fn resolve_api_key_rejects_invalid_env_var_name_without_leaking_it() {
    let _g = env_guard();
    // Simulates a user accidentally pasting the key into api_key_env.
    let bad = "sk-kimi-very-real-looking-pasted-secret";
    let err = resolve_api_key("Kimi", bad, None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("invalid") && msg.contains("api_key_env"),
        "error should explain misconfiguration: {msg}"
    );
    assert!(
        !msg.contains(bad),
        "error must not echo the misconfigured value: {msg}"
    );
    assert!(msg.contains("valid environment variable name"));
    assert!(
        msg.contains("[kimi]"),
        "error should point at the lowercase TOML section: {msg}"
    );
}

#[test]
fn resolve_api_key_invalid_env_name_falls_back_to_inline() {
    let _g = env_guard();
    let got = resolve_api_key("Kimi", "sk-pasted-secret", Some("inline-key")).unwrap();
    assert_eq!(got, "inline-key");
}

#[test]
fn resolve_api_key_never_leaks_valid_looking_configured_env_name() {
    let _g = env_guard();
    // This is syntactically a valid environment variable name, but could
    // be a pasted secret and must not be reflected in the error.
    let pasted_secret = "sk_pasted_secret";
    unsafe { std::env::remove_var(pasted_secret) };
    let err = resolve_api_key("Kimi", pasted_secret, None).unwrap_err();
    assert!(
        !err.to_string().contains(pasted_secret),
        "error must not echo configured api_key_env values"
    );
}

#[test]
fn is_valid_env_var_name_rules() {
    // Valid: alphabetic or underscore first, then alnum/underscore.
    for valid in ["KIMI_API_KEY", "_PRIVATE", "a", "Z9", "MY_ZAI_2"] {
        assert!(is_valid_env_var_name(valid), "{valid} should be valid");
    }
    // Invalid: empty, digit-first, or shell-illegal characters.
    for invalid in ["", "9LIVES", "sk-kimi", "MY KEY", "A.B", "sk/k"] {
        assert!(
            !is_valid_env_var_name(invalid),
            "{invalid} should be invalid"
        );
    }
}

#[test]
fn config_parses_with_inline_api_key_and_primary() {
    let f = write_toml(
        r#"
        [ui]
        primary = "openrouter"

        [zai]
        enabled = true
        api_key_env = "MY_ZAI"
        api_key = "sk-zai-inline"

        [openrouter]
        enabled = true
        api_key = "sk-or-inline"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert_eq!(c.ui.primary, Some(VendorId::Openrouter));
    assert_eq!(c.zai.api_key.as_deref(), Some("sk-zai-inline"));
    assert_eq!(c.openrouter.api_key.as_deref(), Some("sk-or-inline"));
}

#[test]
fn openrouter_named_accounts_preserve_the_default_contract() {
    let f = write_toml(
        r#"
        [openrouter]
        enabled = true
        api_key_env = "AI_USAGEBAR_TEST_OR_DEFAULT"
        api_key = "default-inline"
        show_default_account = false

        [[openrouter.accounts]]
        label = "work"
        api_key_env = "OPENROUTER_WORK_API_KEY"

        [[openrouter.accounts]]
        api_key = "personal-inline"
        "#,
    );
    let _g = env_guard();
    unsafe { std::env::remove_var("AI_USAGEBAR_TEST_OR_DEFAULT") };
    let config = Config::load_from(f.path()).unwrap();
    assert!(!config.openrouter.show_default_account);
    assert_eq!(config.openrouter.accounts.len(), 2);
    assert_eq!(
        config.openrouter.resolve_api_key(None).unwrap(),
        "default-inline"
    );
    // Positional addressing: the hand-written "work" label is ignored,
    // "1"/"2" address the entries in file order.
    let missing = config
        .openrouter
        .resolve_api_key(Some("1"))
        .unwrap_err()
        .to_string();
    assert!(
        missing.contains("OpenRouter account \"1\"") && missing.contains("no API key"),
        "{missing}"
    );
    assert!(!missing.contains("personal-inline"));
    assert_eq!(
        config.openrouter.resolve_api_key(Some("2")).unwrap(),
        "personal-inline"
    );
    assert!(
        config.openrouter.account("work").is_err(),
        "legacy labels no longer address"
    );
}

#[test]
fn key_show_default_fallbacks_are_pinned() {
    // A key vendor reads its own show_default_account field; the default is
    // true.
    let f = write_toml(
        r#"
        [kimi]
        enabled = true

        [[kimi.accounts]]
        api_key = "sk-kimi-1"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.key_show_default(VendorId::Kimi));

    // The same vendor with the field flipped: the field wins.
    let f = write_toml(
        r#"
        [kimi]
        enabled = true
        show_default_account = false

        [[kimi.accounts]]
        api_key = "sk-kimi-1"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(!c.key_show_default(VendorId::Kimi));

    // zai joins the key lookups with accounts: None, so the dispatch
    // falls back to the historical true even when its own field is false.
    let f = write_toml(
        r#"
        [zai]
        enabled = true
        show_default_account = false
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.key_show_default(VendorId::Zai));

    // A vendor outside the key sections entirely (the OAuth path) also
    // falls back to true.
    let f = write_toml("[anthropic]\nenabled = true\n");
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.key_show_default(VendorId::Anthropic));
}

#[test]
fn openrouter_named_accounts_reject_ambiguous_or_unsafe_labels() {
    // Positional naming removed every label hazard: duplicates, path
    // separators, and keyless entries all LOAD now — the position owns
    // the identity, and a keyless account simply reads as not
    // configured at fetch time (the calm state, like every vendor).
    for source in [
        r#"
        [[openrouter.accounts]]
        label = "work"
        api_key = "one"
        [[openrouter.accounts]]
        label = "work"
        api_key = "two"
        "#,
        r#"
        [[openrouter.accounts]]
        label = "../work"
        api_key = "one"
        "#,
        r#"
        [[openrouter.accounts]]
        "#,
    ] {
        let f = write_toml(source);
        let config = Config::load_from(f.path()).unwrap();
        for account in &config.openrouter.accounts {
            assert!(
                !account.label.contains('/') && !account.label.is_empty(),
                "derived labels are safe positions: {:?}",
                account.label
            );
        }
    }
}

#[test]
fn openrouter_unknown_account_never_falls_back_to_default_key() {
    let mut config = OpenRouterConfig {
        api_key: Some("default-secret".into()),
        ..OpenRouterConfig::default()
    };
    config.accounts.push(OpenRouterAccount {
        label: "work".into(),
        api_key_env: None,
        api_key: Some("work-secret".into()),
        monthly_limit: None,
    });
    let message = config
        .resolve_api_key(Some("missing"))
        .unwrap_err()
        .to_string();
    assert!(message.contains("missing") && message.contains("work"));
    assert!(!message.contains("default-secret"));
    assert!(!message.contains("work-secret"));
}

#[test]
fn openrouter_account_key_errors_do_not_echo_configured_values() {
    let config = OpenRouterConfig {
        accounts: vec![OpenRouterAccount {
            label: "work".into(),
            api_key_env: Some("sk_pasted_secret".into()),
            api_key: None,
            monthly_limit: None,
        }],
        ..OpenRouterConfig::default()
    };
    let _g = env_guard();
    unsafe { std::env::remove_var("sk_pasted_secret") };
    let message = config
        .resolve_api_key(Some("work"))
        .unwrap_err()
        .to_string();
    assert!(message.contains("[[openrouter.accounts]]"));
    assert!(!message.contains("sk_pasted_secret"));
}

#[test]
fn enabled_vendors_preserves_canonical_order() {
    // DeepSeek and Kimi are disabled by default (require explicit API key
    // config), so they are absent from the enabled list unless enabled.
    let c = Config::default();
    assert_eq!(
        c.enabled_vendors(),
        vec![
            VendorId::Anthropic,
            VendorId::Openai,
            VendorId::Zai,
            VendorId::Openrouter,
        ]
    );
}

#[test]
fn deepseek_appears_when_enabled() {
    let f = write_toml(
        r#"
        [deepseek]
        enabled = true
        api_key = "sk-test"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.is_enabled(VendorId::Deepseek));
    assert!(c.enabled_vendors().contains(&VendorId::Deepseek));
    assert_eq!(c.deepseek.api_key.as_deref(), Some("sk-test"));
}

#[test]
fn tilde_paths_are_expanded_on_load() {
    // `PathBuf` keeps `~` literally, so the documented
    // `credentials_path = "~/..."` used to resolve to a directory named
    // `~` relative to the process's cwd.
    let f = write_toml(
        r#"
        [context]
        projects_path = "~/.claude/projects"

        [anthropic]
        credentials_path = "~/.claude/.credentials.json"

        [[anthropic.accounts]]
        label = "work"
        credentials_path = "~/work.json"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    let home = crate::cache::home_dir().unwrap();

    assert_eq!(c.context.projects_path, Some(home.join(".claude/projects")));
    let got = c.anthropic.credentials_path.unwrap();
    assert_eq!(got, home.join(".claude/.credentials.json"));
    assert!(!got.to_string_lossy().contains('~'));
    assert_eq!(
        c.anthropic.accounts[0].credentials_path,
        home.join("work.json")
    );
}

#[test]
fn absolute_and_relative_paths_are_left_alone() {
    let f = write_toml(
        r#"
        [anthropic]
        credentials_path = "/etc/creds.json"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert_eq!(
        c.anthropic.credentials_path.unwrap(),
        std::path::Path::new("/etc/creds.json")
    );

    // `~user` is not ours to interpret.
    let f2 = write_toml(
        r#"
        [anthropic]
        credentials_path = "~someone/creds.json"
        "#,
    );
    let c2 = Config::load_from(f2.path()).unwrap();
    assert_eq!(
        c2.anthropic.credentials_path.unwrap(),
        std::path::Path::new("~someone/creds.json")
    );
}

/// The seeded template is documentation, never behavior: every explicit
/// (uncommented) value in `config.example.toml` must equal the code
/// default, or a fresh install with the seed would behave differently
/// from one without a config at all.
#[test]
fn the_example_config_parses_to_pure_defaults() {
    let seeded: Config = toml::from_str(EXAMPLE_CONFIG).expect("config.example.toml must parse");
    assert_eq!(
        serde_json::to_value(&seeded).unwrap(),
        serde_json::to_value(Config::default()).unwrap(),
        "config.example.toml drifted from Config::default()"
    );
}

#[test]
fn seeding_writes_the_annotated_template_once_and_tight() {
    let td = tempfile::TempDir::new().unwrap();
    let path = td.path().join("nested").join("config.toml");

    assert!(seed_example_config(&path).unwrap(), "first call seeds");
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, EXAMPLE_CONFIG);
    assert!(content.contains("[zai]"), "template carries sections");
    assert!(
        content.contains("account_type"),
        "template documents the team fields"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the template starts 0600");
    }

    // Second call: never rewrites — the user may have pasted keys by now.
    std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
    assert!(
        !seed_example_config(&path).unwrap(),
        "existing file is kept"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("primary = \"zai\"")
    );

    // What got seeded parses.
    std::fs::remove_file(&path).unwrap();
    seed_example_config(&path).unwrap();
    Config::load_from(&path).unwrap();
}

#[test]
fn resolved_path_is_the_canonical_one_and_names_the_config_file() {
    // Hermetic: only asserts the shape, never which file happens to exist
    // on the machine running the tests.
    let p = resolved_path().expect("a config path must resolve");
    assert!(p.ends_with("config.toml"));
    let mut candidates = vec![default_path().unwrap()];
    candidates.extend(pre_rename_config_paths());
    assert!(
        candidates.contains(&p),
        "resolved to an unexpected location: {}",
        p.display()
    );
    // The canonical name carries the fork's own directory, and every
    // fallback is a pre-rename spelling — the two installs never meet.
    assert!(
        default_path()
            .unwrap()
            .ends_with("ai-usagebar-omarchy/config.toml")
    );
    for legacy in pre_rename_config_paths() {
        assert!(legacy.ends_with("ai-usagebar/config.toml"), "{legacy:?}");
    }
}

/// The rename migration, hermetically: old layout moves to the new name,
/// modes ride along, nothing is deleted or clobbered.
#[test]
fn migrate_moves_the_old_directory_once_and_preserves_modes() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let old = root.join("ai-usagebar");
    let new = root.join("ai-usagebar-omarchy");
    std::fs::create_dir_all(old.join("accounts").join("work")).unwrap();
    std::fs::write(old.join("config.toml"), "[ui]\nprimary = \"zai\"\n").unwrap();
    std::fs::write(old.join("credentials.json"), "{}").unwrap();
    std::fs::write(
        old.join("accounts").join("work").join(".credentials.json"),
        "{}",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            old.join("credentials.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    assert!(migrate_dir_at(&old, &new).unwrap(), "first call migrates");
    assert!(new.join("config.toml").exists());
    assert!(new.join("credentials.json").exists());
    assert!(new.join("accounts/work/.credentials.json").exists());
    assert!(!old.exists(), "rename removes the old directory");
    assert_eq!(
        std::fs::read_to_string(new.join("config.toml")).unwrap(),
        "[ui]\nprimary = \"zai\"\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(new.join("credentials.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "credential mode rides along");
    }

    // Second call: the new directory exists — never touched again.
    std::fs::write(new.join("config.toml"), "[ui]\nprimary = \"kimi\"\n").unwrap();
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("config.toml"), "upstream lives here now").unwrap();
    assert!(!migrate_dir_at(&old, &new).unwrap(), "existing new is kept");
    assert!(
        std::fs::read_to_string(new.join("config.toml"))
            .unwrap()
            .contains("kimi")
    );

    // Nothing to migrate → false (no old directory).
    std::fs::remove_dir_all(&old).unwrap();
    let nowhere = root.join("nowhere");
    assert!(!migrate_dir_at(&old, &nowhere).unwrap());
    assert!(!nowhere.exists());
}

/// When rename is refused the copy fallback runs: files are duplicated to
/// the new name, the old directory is left in place for the sibling that
/// still uses it.
#[test]
fn migrate_copy_fallback_leaves_the_old_directory_in_place() {
    let td = tempfile::TempDir::new().unwrap();
    let old = td.path().join("ai-usagebar");
    let new = td.path().join("nest").join("ai-usagebar-omarchy");
    std::fs::create_dir_all(old.join("zai").join("team")).unwrap();
    std::fs::write(old.join("config.toml"), "[zai]\nenabled = true\n").unwrap();
    std::fs::write(old.join("zai").join("team").join("usage.json"), "{}").unwrap();

    copy_dir_recursively(&old, &new).unwrap();
    assert_eq!(
        std::fs::read_to_string(new.join("config.toml")).unwrap(),
        "[zai]\nenabled = true\n"
    );
    assert!(new.join("zai/team/usage.json").exists());
    assert!(old.exists(), "copy never deletes the source");
}

#[test]
fn misspelled_section_is_rejected_not_ignored() {
    // The regression this guards: `[openrouer]` used to parse fine, leave
    // OpenRouter on its defaults, and give the user no hint at all.
    let f = write_toml(
        r#"
        [openrouer]
        enabled = true
        api_key = "sk-or-v1-typo"
        "#,
    );
    let err = Config::load_from(f.path()).unwrap_err().to_string();
    assert!(
        err.contains("openrouer"),
        "error should name the typo: {err}"
    );
}

#[test]
fn invalid_toml_is_an_error_not_silent_defaults() {
    let f = write_toml("[zai\nenabled = true\n");
    assert!(Config::load_from(f.path()).is_err());
}

#[test]
fn a_missing_file_is_still_just_defaults() {
    // Absence stays the legitimate "use defaults" case — only real parse
    // and I/O failures are errors.
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope").join("config.toml");
    let c = Config::load_from(&missing).unwrap();
    assert!(c.is_enabled(VendorId::Anthropic));
}

#[test]
fn kimi_appears_when_enabled() {
    let f = write_toml(
        r#"
        [kimi]
        enabled = true
        api_key = "sk-test"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.is_enabled(VendorId::Kimi));
    assert!(c.enabled_vendors().contains(&VendorId::Kimi));
    assert_eq!(c.kimi.api_key.as_deref(), Some("sk-test"));
}

#[test]
fn enabled_deepseek_and_kimi_appear_in_canonical_order_ending_with_them() {
    let f = write_toml(
        r#"
        [deepseek]
        enabled = true
        api_key = "sk-ds"

        [kimi]
        enabled = true
        api_key = "sk-kimi"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert_eq!(
        c.enabled_vendors(),
        vec![
            VendorId::Anthropic,
            VendorId::Openai,
            VendorId::Zai,
            VendorId::Openrouter,
            VendorId::Deepseek,
            VendorId::Kimi,
        ]
    );
}

#[test]
fn parses_anthropic_accounts_and_looks_them_up() {
    let f = write_toml(
        r#"
        [anthropic]
        enabled = true

        [[anthropic.accounts]]
        label = "personal"
        credentials_path = "/creds/personal.json"

        [[anthropic.accounts]]
        label = "work"
        credentials_path = "/creds/work.json"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert_eq!(c.anthropic.accounts.len(), 2);
    let work = c.anthropic.account("work").unwrap();
    assert_eq!(work.credentials_path, PathBuf::from("/creds/work.json"));
    // A typo names the offending label and lists the known ones.
    let err = format!("{:?}", c.anthropic.account("missing").unwrap_err());
    assert!(err.contains("missing") && err.contains("work"), "{err}");
}

#[test]
fn duplicate_anthropic_account_labels_are_rejected_on_load() {
    let f = write_toml(
        r#"
        [[anthropic.accounts]]
        label = "work"
        credentials_path = "/creds/work-one.json"

        [[anthropic.accounts]]
        label = "work"
        credentials_path = "/creds/work-two.json"
        "#,
    );
    let err = Config::load_from(f.path()).unwrap_err().to_string();
    assert!(
        err.contains("duplicate anthropic account label \"work\""),
        "{err}"
    );
}

#[test]
fn account_label_rejects_path_like_names() {
    let cfg = AnthropicConfig::default();
    for bad in [
        "",
        ".",
        "..",
        "a/b",
        r"a\b",
        "C:work",
        "line\nbreak",
        "tab\tname",
        "usage.json",
        ".stale",
        ".last_error",
        ".fetch.lock",
    ] {
        let err = cfg.account(bad).unwrap_err();
        assert!(
            format!("{err:?}").contains("invalid anthropic account label"),
            "{bad:?} should be rejected as a label"
        );
    }
}

#[test]
fn anthropic_accounts_default_to_empty() {
    // No [[anthropic.accounts]] → the single default account, empty list,
    // nothing to migrate (issue #14, back-compat rule 1).
    assert!(Config::default().anthropic.accounts.is_empty());
    assert!(Config::default().anthropic.accounts_dir.is_none());
}

// --- accounts_dir: CLAUDE_CONFIG_DIR-style auto-discovery ----------------
// All hermetic: discovery reads a TempDir, never the user's real config.

/// Create `<root>/<label>/.credentials.json` (contents irrelevant here —
/// discovery keys on the file existing, the fetch path parses it).
fn seed_account_dir(root: &std::path::Path, label: &str) {
    let dir = root.join(label);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".credentials.json"), "{}").unwrap();
}

#[test]
fn discovers_account_dirs_in_claude_config_dir_layout() {
    let td = tempfile::tempdir().unwrap();
    seed_account_dir(td.path(), "work");
    seed_account_dir(td.path(), "personal");
    // Keychain-backed macOS logins may not write .credentials.json; their
    // config directories are still account entries.
    std::fs::create_dir_all(td.path().join("keychain-only")).unwrap();
    // A loose file (not a dir) is ignored.
    std::fs::write(td.path().join("stray.json"), "{}").unwrap();

    let cfg = AnthropicConfig {
        accounts_dir: Some(td.path().to_path_buf()),
        ..Default::default()
    };
    let all = cfg.all_accounts();
    let labels: Vec<&str> = all.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, vec!["keychain-only", "personal", "work"]);
    assert_eq!(
        all[2].credentials_path,
        td.path().join("work").join(".credentials.json")
    );
}

#[test]
fn explicit_account_wins_over_a_discovered_one_with_the_same_label() {
    let td = tempfile::tempdir().unwrap();
    seed_account_dir(td.path(), "work");
    let cfg = AnthropicConfig {
        accounts: vec![AnthropicAccount {
            label: "work".into(),
            credentials_path: "/explicit/work.json".into(),
        }],
        accounts_dir: Some(td.path().to_path_buf()),
        ..Default::default()
    };
    let all = cfg.all_accounts();
    assert_eq!(all.len(), 1, "no duplicate label");
    assert_eq!(
        all[0].credentials_path,
        std::path::Path::new("/explicit/work.json"),
        "explicit entry wins"
    );
    // A discovered account is still reachable through `account()`.
    seed_account_dir(td.path(), "other");
    assert_eq!(cfg.account("other").unwrap().label, "other");
}

#[test]
fn missing_accounts_dir_is_silently_empty_not_an_error() {
    let cfg = AnthropicConfig {
        accounts_dir: Some("/nonexistent/ai-usagebar-accounts".into()),
        ..Default::default()
    };
    assert!(cfg.all_accounts().is_empty());
}

#[test]
fn accounts_dir_is_tilde_expanded_on_load() {
    let f = write_toml(
        r#"
        [anthropic]
        accounts_dir = "~/.config/ai-usagebar-omarchy/accounts"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    let home = crate::cache::home_dir().unwrap();
    assert_eq!(
        c.anthropic.accounts_dir,
        Some(home.join(".config/ai-usagebar-omarchy/accounts"))
    );
}

#[test]
fn desktop_profiles_dir_is_tilde_expanded_on_load() {
    let f = write_toml(
        r#"
        [anthropic]
        desktop_profiles_dir = "~/.claude-acc/profiles"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    let home = crate::cache::home_dir().unwrap();
    assert_eq!(
        c.anthropic.desktop_profiles_dir,
        Some(home.join(".claude-acc/profiles"))
    );
}

#[test]
fn the_live_cli_account_is_read_from_the_default_credential_slot() {
    let cfg = AnthropicConfig {
        accounts: vec![
            AnthropicAccount {
                label: "work".into(),
                credentials_path: "/tmp/accounts/work/.credentials.json".into(),
            },
            AnthropicAccount {
                label: "personal".into(),
                credentials_path: "/tmp/accounts/personal/.credentials.json".into(),
            },
        ],
        ..Default::default()
    };

    let (idle, idle_cache) = cfg.account_target_with("work", Some("personal")).unwrap();
    assert!(
        matches!(&idle, CredsTarget::Named { config_dir, .. }
            if config_dir == std::path::Path::new("/tmp/accounts/work")),
        "{idle:?}"
    );

    // Same label, but it is the login `claude` itself is using: one lineage.
    let (live, live_cache) = cfg.account_target_with("work", Some("work")).unwrap();
    assert!(matches!(live, CredsTarget::Default(_)), "{live:?}");

    // The cache must not move, or a switch would silently orphan the tab's
    // usage history and show "Loading…" until the next fetch.
    assert_eq!(idle_cache.dir(), live_cache.dir());
}

#[test]
fn no_live_cli_account_keeps_every_account_on_its_own_slot() {
    let cfg = AnthropicConfig {
        accounts: vec![AnthropicAccount {
            label: "work".into(),
            credentials_path: "/tmp/accounts/work/.credentials.json".into(),
        }],
        ..Default::default()
    };
    let (target, _) = cfg.account_target_with("work", None).unwrap();
    assert!(matches!(target, CredsTarget::Named { .. }), "{target:?}");
}

/// The shipped example, which `make install` puts in
/// `share/ai-usagebar/config.example.toml`. Repo-relative, so this stays
/// hermetic — it never touches the user's real config.
fn config_example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml")
}

#[test]
fn shipped_example_parses_as_a_real_config() {
    // The example is documentation users copy verbatim, but nothing used
    // to parse it — so a renamed section or field could rot there
    // unnoticed, and `deny_unknown_fields` would reject the copy on the
    // user's machine instead of in CI.
    let c = Config::load_from(&config_example()).unwrap();
    assert!(!c.context.enabled);
    assert!(c.is_enabled(VendorId::Anthropic));
    assert!(c.is_enabled(VendorId::Openai));
    assert!(!c.is_enabled(VendorId::AnthropicApi));
    assert!(!c.is_enabled(VendorId::Deepseek));
    assert!(!c.is_enabled(VendorId::Kimi));
    assert!(!c.is_enabled(VendorId::Kilo));
    assert!(!c.is_enabled(VendorId::Novita));
    assert!(!c.is_enabled(VendorId::Moonshot));
    assert!(!c.is_enabled(VendorId::Grok));
    assert!(!c.is_enabled(VendorId::Cursor));
    assert!(!c.is_enabled(VendorId::Minimax));
}

#[test]
fn shipped_example_does_not_advertise_admin_key_env_as_working() {
    // The regression: the example shipped an *uncommented*
    // `admin_key_env = "OPENAI_ADMIN_KEY"`, indistinguishable from a live
    // setting. Nothing reads it, so a user could set it, skip
    // `codex login`, and wait for usage that never arrives.
    let text = std::fs::read_to_string(config_example()).unwrap();
    let live: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.contains("admin_key_env") && !l.starts_with('#'))
        .collect();
    assert!(
        live.is_empty(),
        "admin_key_env must stay commented out while it is inert: {live:?}"
    );
    // Still documented, though — silently dropping it would leave users
    // who already set it with no explanation of where that job moved.
    assert!(
        text.contains("admin_key_env") && text.contains("[openai_api]"),
        "the example should keep pointing admin_key_env readers at [openai_api]"
    );
}

#[test]
fn admin_key_env_is_accepted_but_changes_nothing() {
    // The field survives because the API-key-only path is still intended.
    // What has to hold today is narrower: setting it loads without error
    // and moves nothing the code actually acts on.
    let f = write_toml(
        r#"
        [openai]
        admin_key_env = "SOME_ADMIN_KEY"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert_eq!(c.openai.admin_key_env, "SOME_ADMIN_KEY");
    // Nothing else moved: OpenAI still resolves through Codex OAuth only.
    let default = OpenAiConfig::default();
    assert_eq!(c.openai.enabled, default.enabled);
    assert_eq!(c.openai.codex_auth_path, default.codex_auth_path);
    assert_eq!(c.enabled_vendors(), Config::default().enabled_vendors());
}

#[test]
fn config_example_documents_every_vendor_without_secrets() {
    let raw = std::fs::read_to_string(config_example()).unwrap();
    let cfg = Config::load_from(&config_example()).unwrap();
    // Every vendor the binary can dispatch needs a documented section, or
    // users have no way to discover how to turn it on.
    for id in VendorId::all() {
        let section = id.slug();
        assert!(
            raw.contains(&format!("[{section}]")),
            "config.example.toml has no [{section}] section"
        );
    }

    // The example must not ship anything enabled-by-key-only, and must not
    // carry a real secret.
    assert!(!cfg.anthropic_api.enabled && cfg.anthropic_api.api_key.is_none());
    assert!(!cfg.kilo.enabled && cfg.kilo.api_key.is_none());
    assert!(!cfg.novita.enabled && cfg.novita.api_key.is_none());
    assert!(!cfg.moonshot.enabled && cfg.moonshot.api_key.is_none());
    assert!(!cfg.grok.enabled && cfg.grok.api_key.is_none());
    assert!(!cfg.supergrok.enabled);
    assert_eq!(cfg.supergrok.grok_binary, default_grok_binary());
    assert_eq!(
        cfg.supergrok
            .grok_binary
            .file_name()
            .and_then(|p| p.to_str()),
        Some(if cfg!(windows) { "grok.exe" } else { "grok" })
    );
    assert!(cfg.supergrok.auth_path.is_none());
    assert!(cfg.supergrok.config_path.is_none());
    assert!(!cfg.cursor.enabled && cfg.cursor.db_path.is_none());
    assert!(!cfg.kiro.enabled && cfg.kiro.db_path.is_none());
}

#[test]
fn supergrok_binary_must_not_be_empty() {
    let file = write_toml(
        r#"
        [supergrok]
        enabled = true
        grok_binary = ""
        "#,
    );
    let error = Config::load_from(file.path()).unwrap_err().to_string();
    assert!(error.contains("grok_binary must not be empty"));
}

#[test]
fn supergrok_paths_are_tilde_expanded() {
    let file = write_toml(
        r#"
        [supergrok]
        grok_binary = "~/bin/grok"
        auth_path = "~/.grok/auth.json"
        config_path = "~/.grok/config.toml"
        "#,
    );
    let config = Config::load_from(file.path()).unwrap();
    let home = crate::cache::home_dir().unwrap();
    assert_eq!(config.supergrok.grok_binary, home.join("bin/grok"));
    assert_eq!(
        config.supergrok.auth_path,
        Some(home.join(".grok/auth.json"))
    );
    assert_eq!(
        config.supergrok.config_path,
        Some(home.join(".grok/config.toml"))
    );
}

#[test]
fn kiro_db_path_is_tilde_expanded() {
    let f = write_toml(
        r#"
        [kiro]
        db_path = "~/kiro-data.sqlite3"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    let home = crate::cache::home_dir().unwrap();
    assert_eq!(c.kiro.db_path, Some(home.join("kiro-data.sqlite3")));
}

#[test]
fn kiro_appears_when_enabled() {
    let f = write_toml(
        r#"
        [kiro]
        enabled = true
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.is_enabled(VendorId::Kiro));
    assert!(c.enabled_vendors().contains(&VendorId::Kiro));
}

#[test]
fn cursor_db_path_is_tilde_expanded() {
    let f = write_toml(
        r#"
        [cursor]
        db_path = "~/cursor-state.vscdb"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    let home = crate::cache::home_dir().unwrap();
    assert_eq!(c.cursor.db_path, Some(home.join("cursor-state.vscdb")));
}

#[test]
fn cursor_agent_auth_path_is_tilde_expanded() {
    let f = write_toml(
        r#"
        [cursor]
        agent_auth_path = "~/cursor-agent-auth.json"
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    let home = crate::cache::home_dir().unwrap();
    assert_eq!(
        c.cursor.agent_auth_path,
        Some(home.join("cursor-agent-auth.json"))
    );
}

#[test]
fn cursor_appears_when_enabled() {
    let f = write_toml(
        r#"
        [cursor]
        enabled = true
        "#,
    );
    let c = Config::load_from(f.path()).unwrap();
    assert!(c.is_enabled(VendorId::Cursor));
    assert!(c.enabled_vendors().contains(&VendorId::Cursor));
}

#[test]
fn add_account_appends_and_preserves_existing() {
    let mut doc: toml_edit::DocumentMut = r#"
# keep me
[anthropic]
enabled = true

[[anthropic.accounts]]
label = "personal"
credentials_path = "~/.config/ai-usagebar-omarchy/accounts/personal/.credentials.json"
"#
    .parse()
    .unwrap();
    add_anthropic_account_to_doc(
        &mut doc,
        "work",
        "~/.config/ai-usagebar-omarchy/accounts/work/.credentials.json",
    )
    .unwrap();
    let rendered = doc.to_string();
    assert!(rendered.contains("# keep me"), "comment must survive");
    // Round-trips through the real loader with both accounts intact and ordered.
    let f = write_toml(&rendered);
    let c = Config::load_from(f.path()).unwrap();
    let labels: Vec<&str> = c
        .anthropic
        .accounts
        .iter()
        .map(|a| a.label.as_str())
        .collect();
    assert_eq!(labels, vec!["personal", "work"]);
}

#[test]
fn add_account_to_empty_doc_is_loadable() {
    let mut doc = toml_edit::DocumentMut::new();
    add_anthropic_account_to_doc(&mut doc, "solo", "~/x/.credentials.json").unwrap();
    let f = write_toml(&doc.to_string());
    let c = Config::load_from(f.path()).unwrap();
    assert_eq!(c.anthropic.accounts.len(), 1);
    assert_eq!(c.anthropic.accounts[0].label, "solo");
}

#[test]
fn add_account_rejects_duplicate_label() {
    let mut doc: toml_edit::DocumentMut = r#"
[[anthropic.accounts]]
label = "work"
credentials_path = "~/w/.credentials.json"
"#
    .parse()
    .unwrap();
    assert!(
        add_anthropic_account_to_doc(&mut doc, "work", "~/other/.credentials.json").is_err(),
        "a duplicate label must be rejected, not appended"
    );
}

#[test]
fn add_account_rejects_bad_label() {
    let mut doc = toml_edit::DocumentMut::new();
    assert!(add_anthropic_account_to_doc(&mut doc, "a/b", "~/x/.credentials.json").is_err());
    assert!(add_anthropic_account_to_doc(&mut doc, "", "~/x/.credentials.json").is_err());
}

#[test]
fn tildify_collapses_home_only() {
    let home = Path::new("/Users/me");
    assert_eq!(tildify(&home.join("a/b"), home), "~/a/b");
    assert_eq!(tildify(Path::new("/etc/hosts"), home), "/etc/hosts");
}

#[test]
fn default_account_credentials_path_nests_under_config_dir() {
    let cfg = Path::new("/home/u/.config/ai-usagebar/config.toml");
    assert_eq!(
        default_account_credentials_path(cfg, "work"),
        Path::new("/home/u/.config/ai-usagebar/accounts/work/.credentials.json"),
    );
}
