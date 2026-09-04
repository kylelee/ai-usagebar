use std::path::Path;

use super::resolved_path;
use crate::error::{AppError, Result};

/// The annotated example configuration shipped with the source, embedded so
/// every install method (Omarchy plugin, AUR, crates.io, `cargo install`)
/// can seed a fillable template without distributing a second copy of the
/// file. Behavioral contract: it must parse to exactly [`Config::default`]
/// — a guarded by test — so seeding it changes documentation, never
/// behavior.
pub const EXAMPLE_CONFIG: &str = include_str!("../../config.example.toml");

/// Write [`EXAMPLE_CONFIG`] to `path` when (and only when) nothing exists
/// there yet — a fresh install's first run turns "where do I configure
/// this?" into an annotated file to fill in. Never overwrites: the file may
/// hold the user's API keys. Created 0600 because that is what it is for.
/// Returns whether the template was written.
pub fn seed_example_config(path: &Path) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| AppError::io_at(dir, e))?;
    // `tempfile` creates 0600 and `persist` keeps the mode — start tight,
    // the template's own header still tells users to keep it that way.
    let mut tmp = tempfile::Builder::new()
        .prefix(".config.")
        .tempfile_in(dir)
        .map_err(|e| AppError::io_at(dir, e))?;
    std::io::Write::write_all(&mut tmp, EXAMPLE_CONFIG.as_bytes())
        .map_err(|e| AppError::io_at(tmp.path(), e))?;
    tmp.persist(path)
        .map_err(|e| AppError::io_at(path, e.error))?;
    Ok(true)
}

/// Resolved `config.toml` path as a string for user-facing messages. Uses the
/// platform's config dir (`directories::ProjectDirs`), so it reads correctly on
/// Linux, macOS, and Windows instead of hard-coding the Unix `~/.config` path.
/// Falls back to the bare filename if the path can't be resolved.
pub fn config_path_hint() -> String {
    resolved_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "config.toml".to_string())
}
