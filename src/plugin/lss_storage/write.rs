#[cfg(test)]
mod tests;

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use classicube_helpers::tab_list::remove_color;
use livesplit_core::{Run, Segment, run::saver::livesplit::save_run};
use tracing::{debug, info};

use crate::{
    chat::chat_print_async,
    plugin::{
        lss_storage::{
            path,
            payload::{self, CUSTOM_VAR_NAME},
        },
        splits::geometry::Track,
    },
};

/// Persist `track` to the on-disk store for `(server, map)`. Spawned
/// as a tokio task so blocking `fs` calls don't stall the game tick.
/// Errors are surfaced via chat-print; the function itself never panics.
pub async fn save_track(track: Track, server_display: String, map: String) {
    let dir = path::track_dir(&server_display, &map);
    let category = path::sanitize_component(&track.name);

    match save_track_to(&track, &server_display, &dir, &category) {
        Ok(SaveOutcome::Wrote(path)) => {
            info!(?path, "wrote new track version");
            let filename = path
                .file_name()
                .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            chat_print_async(format!("&aLiveSplit: saved {filename}"));
        }
        Ok(SaveOutcome::AlreadyLatest) => {
            debug!("track unchanged from latest on-disk version; no write");
        }
        Err(e) => {
            chat_print_async(format!("&cLiveSplit: failed to save track: {e:#}"));
        }
    }
}

pub(super) enum SaveOutcome {
    Wrote(PathBuf),
    AlreadyLatest,
}

pub(super) fn save_track_to(
    track: &Track,
    server_display: &str,
    dir: &Path,
    category: &str,
) -> Result<SaveOutcome> {
    let canonical = payload::serialize_canonical(track).context("serializing payload")?;

    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let versions = path::list_versions(dir, category);
    if let Some((_v, ref latest_path)) = versions.last().cloned()
        && same_as_latest(latest_path, &canonical)?
    {
        return Ok(SaveOutcome::AlreadyLatest);
    }

    let next_version = versions.last().map_or(1u32, |(v, _)| v.saturating_add(1));
    let final_path = dir.join(format!("{category}-v{next_version}.lss"));
    let tmp_path = dir.join(format!("{category}-v{next_version}.lss.tmp"));

    let canonical_str =
        std::str::from_utf8(&canonical).context("canonical payload is not valid UTF-8")?;
    let xml = build_lss_xml(track, server_display, canonical_str)?;

    fs::write(&tmp_path, xml.as_bytes())
        .with_context(|| format!("writing {}", tmp_path.display()))?;

    if let Err(e) = fs::rename(&tmp_path, &final_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e).with_context(|| {
            format!(
                "renaming {} -> {}",
                tmp_path.display(),
                final_path.display()
            )
        });
    }

    Ok(SaveOutcome::Wrote(final_path))
}

fn same_as_latest(path: &Path, canonical: &[u8]) -> Result<bool> {
    let xml = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            debug!(?path, error = ?e, "could not read latest version; will write new");
            return Ok(false);
        }
    };
    let run = match livesplit_core::run::parser::livesplit::parse(&xml) {
        Ok(r) => r,
        Err(e) => {
            debug!(?path, error = ?e, "could not parse latest version; will write new");
            return Ok(false);
        }
    };
    let stored = run
        .metadata()
        .custom_variable_value(CUSTOM_VAR_NAME)
        .unwrap_or("");
    Ok(stored.as_bytes() == canonical)
}

fn build_lss_xml(track: &Track, server_display: &str, canonical_json: &str) -> Result<String> {
    let mut run = Run::new();
    run.set_game_name("ClassiCube");

    let server_pretty = remove_color(server_display);
    let track_pretty = remove_color(&track.name);
    run.set_category_name(format!("{server_pretty} - {track_pretty}"));

    // LiveSplit's segment list is everything after the implicit Start:
    // pressing Start is the timer-side run-start action, not a named
    // split (you run *into* the first split). So the Start checkpoint at
    // index 0 doesn't get a `<Segment>`; the rest do, in order. The
    // reader mirrors this -- it expects one label per non-Start
    // checkpoint and defaults the Start's label.
    for cp in track.checkpoints.iter().skip(1) {
        run.push_segment(Segment::new(cp.label.as_str()));
    }

    run.metadata_mut()
        .custom_variable_mut(CUSTOM_VAR_NAME)
        .permanent()
        .set_value(canonical_json);

    let mut xml = String::new();
    save_run(&run, &mut xml).map_err(|e| anyhow!("saving Run to XML: {e}"))?;
    Ok(xml)
}
