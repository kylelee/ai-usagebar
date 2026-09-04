use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

pub fn default_path() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", crate::APP_DIR)?;
    Some(proj.config_dir().join("config.toml"))
}

/// Config files that predate the `ai-usagebar` → `ai-usagebar-omarchy` fork
/// rename, in precedence order. Both spellings existed in the wild: the
/// upstream ProjectDirs location (`~/Library/Application Support/ai-usagebar`
/// on macOS, `~/.config/ai-usagebar` on Linux — and its `$XDG_CONFIG_HOME`
/// equivalent) and the Unix-conventional `~/.config/ai-usagebar` that every
/// doc and the config example always pointed at. On Linux the two coincide,
/// which is harmless.
pub(super) fn pre_rename_config_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(proj) = directories::ProjectDirs::from("", "", crate::LEGACY_APP_DIR) {
        out.push(proj.config_dir().join("config.toml"));
    }
    if let Ok(home) = crate::cache::home_dir() {
        out.push(
            home.join(".config")
                .join(crate::LEGACY_APP_DIR)
                .join("config.toml"),
        );
    }
    out
}

/// The config file actually in effect.
///
/// [`default_path`] stays canonical; a config left at a pre-rename location
/// is honored in place when the canonical one does not exist — normally only
/// briefly, because [`Config::load`] first attempts
/// [`migrate_legacy_layout`] to move the whole directory (config,
/// credentials, accounts) out of the upstream project's reach. The legacy
/// file is never *rewritten*: it may hold API keys, and relocating a secret
/// behind the user's back is not this tool's business — moving the directory
/// is the user's own rename decision, already expressed by installing this
/// fork.
pub fn resolved_path() -> Option<PathBuf> {
    let canonical = default_path();
    if let Some(p) = &canonical
        && p.exists()
    {
        return canonical;
    }
    for legacy in pre_rename_config_paths() {
        if legacy.exists() {
            return Some(legacy);
        }
    }
    canonical
}

/// One-time directory migration for the fork rename: move
/// `~/.config/ai-usagebar` → `~/.config/ai-usagebar-omarchy` and
/// `~/.cache/ai-usagebar` → `~/.cache/ai-usagebar-omarchy` so this project
/// stops sharing state with the upstream `ai-usagebar` it forked from (two
/// installs would otherwise overwrite each other's config edits and race the
/// same cache locks with different payload schemas).
///
/// Semantics: runs only when the new directory does not exist and an old one
/// does; renames (same filesystem, instant), falling back to a recursive
/// copy that preserves permissions (0600 credentials stay 0600) when a
/// cross-filesystem home or a running sibling makes the rename fail. Nothing
/// is ever deleted and nothing existing is ever overwritten, so a concurrent
/// process that already migrated makes this a no-op. Best-effort: any
/// failure leaves the old layout in place, still read-honored by
/// [`resolved_path`].
pub fn migrate_legacy_layout() {
    if let Some(new_dir) = default_path().and_then(|p| p.parent().map(Path::to_path_buf)) {
        for old in pre_rename_config_paths() {
            let Some(old_dir) = old.parent() else {
                continue;
            };
            match migrate_dir_at(old_dir, &new_dir) {
                Ok(true) => {
                    crate::diag::event(
                        "config/migrate",
                        &format!("{} -> {}", old_dir.display(), new_dir.display()),
                    );
                    break;
                }
                Err(e) => crate::diag::event(
                    "config/migrate",
                    &format!("{} failed: {}", old_dir.display(), e.user_message()),
                ),
                _ => {}
            }
        }
    }
    if let Ok(new_cache) = crate::cache::app_cache_root()
        && let Ok(old_cache) = crate::cache::legacy_app_cache_root()
    {
        let _ = migrate_dir_at(&old_cache, &new_cache);
    }
}

/// Move `old` to `new` once: `false` when `new` already exists (migrated, or
/// a fresh install's own directory — never clobber it) or `old` is absent.
/// Rename first; copy recursively as the fallback, leaving `old` in place.
pub(super) fn migrate_dir_at(old: &Path, new: &Path) -> Result<bool> {
    if new.exists() || !old.exists() {
        return Ok(false);
    }
    if std::fs::rename(old, new).is_ok() {
        return Ok(true);
    }
    // Rename refused (cross-filesystem home is the usual reason; a sibling
    // process having just completed the migration shows up as `old` gone).
    if !old.exists() {
        return Ok(false);
    }
    copy_dir_recursively(old, new).map(|_| true)
}

pub(super) fn copy_dir_recursively(old: &Path, new: &Path) -> Result<()> {
    std::fs::create_dir_all(new).map_err(|e| AppError::io_at(new, e))?;
    let entries = match std::fs::read_dir(old) {
        Ok(entries) => entries,
        // The source vanished mid-migration (a concurrent rename won): the
        // files that mattered already reached `new`.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io_at(old, e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| AppError::io_at(old, e))?;
        let from = entry.path();
        let to = new.join(entry.file_name());
        // `fs::copy` preserves permissions, so 0600 credential files and the
        // config the user already chmod'd keep their modes.
        if entry
            .file_type()
            .map_err(|e| AppError::io_at(&from, e))?
            .is_dir()
        {
            copy_dir_recursively(&from, &to)?;
        } else if !to.exists() {
            std::fs::copy(&from, &to).map_err(|e| AppError::io_at(&from, e))?;
        }
    }
    Ok(())
}
