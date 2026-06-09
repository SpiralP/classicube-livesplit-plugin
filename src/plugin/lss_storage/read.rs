#[cfg(test)]
mod tests;

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, anyhow, bail};
use classicube_helpers::async_manager;
use livesplit_core::Run;
use tracing::{debug, info};

use crate::{
    chat::chat_print_async,
    plugin::{
        editor,
        lss_storage::{
            path,
            payload::{self, CUSTOM_VAR_NAME},
        },
        splits::{self, geometry::Track},
    },
};

/// Attempt to auto-load the most recently modified `.lss` file from
/// the on-disk store for `(server, map)`. Soft-fails on missing
/// directory; chat-prints on parse failures so user-introduced errors
/// (e.g. hand-edited custom variable) surface.
pub async fn try_autoload(server: String, map: String) {
    match try_autoload_inner(&server, &map) {
        Ok(None) => {
            debug!("autoload: no track files for ({server}, {map})");
        }
        Ok(Some((track, path))) => {
            async_manager::spawn_on_main_thread(async move {
                // Re-check on the main thread: the disk read + thread
                // hop is a sub-second window in which the player can
                // step into a Start AABB, which would make the
                // autoload clobber a just-started run
                // (`SplitsState::load` resets `next_index`/`fired[]`).
                if splits::run_in_progress() {
                    debug!("autoload skipped on main thread: run started during disk read");
                    return;
                }
                // Close the race between the tick-side editor guard and
                // the async hop: if the user enabled edit mode *after*
                // the tick already spawned this task, suppress the load
                // just like the tick would have.
                if editor::is_enabled() && splits::track_includes_map(&map) {
                    debug!(
                        "autoload skipped on main thread: editing a track that includes this map"
                    );
                    return;
                }
                // `false` means the plugin is mid-teardown
                // (`SplitsState::load` returned `None` because `STATE`
                // was cleared); nothing actionable beyond logging.
                if splits::load_track(track, "disk") {
                    super::set_loaded_path(path);
                } else {
                    debug!("autoload: load_track returned false (plugin mid-teardown)");
                }
            });
        }
        Err(e) => {
            debug!("autoload skipped: {e:#}");
        }
    }
}

/// On-demand load for `/client LiveSplit load [filename]`. `None` picks
/// the newest `.lss` by mtime (same selection as autoload); `Some(name)`
/// loads that specific file from the `(server, map)` directory. Unlike
/// autoload, this surfaces the not-found / parse-error cases to chat and
/// is NOT gated on `run_in_progress()` (the command is explicit). When
/// the load aborts an in-progress run, a connected timer is reset so it
/// doesn't keep running against stale segments.
pub async fn load_command(server: String, map: String, filename: Option<String>) {
    match resolve_for_load(&server, &map, filename.as_deref()) {
        Ok((track, source, path)) => {
            async_manager::spawn_on_main_thread(async move {
                // `false` => plugin mid-teardown; nothing loaded.
                // `load_track` itself resets a connected timer if the load
                // aborts an in-progress run.
                if splits::load_track(track, &source) {
                    super::set_loaded_path(path);
                }
            });
        }
        Err(e) => chat_print_async(format!("&cLiveSplit: {e:#}")),
    }
}

/// Resolve the file to load and parse it into a `Track`. Returns the
/// track, its file basename (used as the `source` label in the
/// `load_track` success chat line), and the chosen file path (remembered
/// via `set_loaded_path` so `/client LiveSplit open` can reveal it).
/// Errors carry a user-facing message.
fn resolve_for_load(
    server: &str,
    map: &str,
    filename: Option<&str>,
) -> Result<(Track, String, PathBuf)> {
    let dir = path::track_dir(server, map);
    let chosen = match filename {
        None => newest_lss(&dir)?
            .ok_or_else(|| anyhow!("no track files for this map ({})", dir.display()))?,
        Some(name) => {
            let fname =
                normalize_lss_filename(name).ok_or_else(|| anyhow!("invalid filename '{name}'"))?;
            let path = dir.join(&fname);
            if !path.exists() {
                let avail = available_lss(&dir);
                if avail.is_empty() {
                    bail!("file not found: {fname}");
                }
                bail!("file not found: {fname} (available: {})", avail.join(", "));
            }
            path
        }
    };
    let basename = chosen.file_name().map_or_else(
        || chosen.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let track = load_from_file(&chosen)?;
    Ok((track, basename, chosen))
}

/// Sorted `.lss` basenames in `dir`, for the "available:" hint on a
/// failed named load. Empty when the directory is missing/unreadable.
fn available_lss(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("lss") {
                p.file_name().map(|n| n.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// Reject a user-supplied load filename that escapes the track dir, and
/// append `.lss` when missing. Returns `None` if the name contains any
/// path component beyond a single file name (`/`, `\`, `..`, `.`, an
/// absolute path).
fn normalize_lss_filename(name: &str) -> Option<String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return None;
    }
    // After ruling out separators, the name must still be exactly its
    // own final component -- `Path::file_name` is `None` for `.`/`..`,
    // catching those traversal forms on every platform.
    if Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name) {
        return None;
    }
    if name.to_ascii_lowercase().ends_with(".lss") {
        Some(name.to_owned())
    } else {
        Some(format!("{name}.lss"))
    }
}

fn try_autoload_inner(server: &str, map: &str) -> Result<Option<(Track, PathBuf)>> {
    let dir = path::track_dir(server, map);
    if !dir.exists() {
        return Ok(None);
    }

    let Some(newest) = newest_lss(&dir)? else {
        return Ok(None);
    };

    match load_from_file(&newest) {
        Ok(track) => {
            info!(path = %newest.display(), "auto-loaded track from disk");
            Ok(Some((track, newest)))
        }
        Err(e) => {
            let basename = newest.file_name().map_or_else(
                || newest.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            chat_print_async(format!("&eLiveSplit: skipping {basename}: {e:#}"));
            Ok(None)
        }
    }
}

fn newest_lss(dir: &Path) -> Result<Option<PathBuf>> {
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lss") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        match best {
            Some((cur, _)) if cur >= mtime => {}
            _ => best = Some((mtime, path)),
        }
    }
    Ok(best.map(|(_, p)| p))
}

fn load_from_file(path: &Path) -> Result<Track> {
    let xml = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let run = livesplit_core::run::parser::livesplit::parse(&xml)
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    let Some(value) = run.metadata().custom_variable_value(CUSTOM_VAR_NAME) else {
        bail!("no ClassiCubeTrack custom variable");
    };
    // The title (track/category name) lives in the `.lss` `<CategoryName>`,
    // not the geometry payload; `payload::parse` overwrites the decoded
    // empty name with it.
    let title = run.category_name().to_owned();
    let labels = labels_from(&run);
    payload::parse(value, title, labels).context("parsing ClassiCubeTrack payload")
}

fn labels_from(run: &Run) -> Vec<String> {
    run.segments().iter().map(|s| s.name().to_owned()).collect()
}
