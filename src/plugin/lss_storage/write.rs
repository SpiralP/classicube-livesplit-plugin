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
            payload::{self, CUSTOM_VAR_NAME, MAP_VAR_NAME, SERVER_VAR_NAME},
        },
        splits::geometry::Track,
        track_source::decode,
    },
};

/// Persist `track` to the on-disk store for `(server, map)`. Spawned
/// as a tokio task so blocking `fs` calls don't stall the game tick.
/// Errors are surfaced via chat-print; the function itself never panics.
///
/// A new on-disk version always chat-prints the written filename (both
/// the background autosave and the manual command). `announce` only
/// controls the *no-op* case: the manual `/client LiveSplit save`
/// command passes `true` so the user gets "already saved (no changes)"
/// feedback when the dedup gate skips the write; the background autosave
/// passes `false` to stay silent on no-op.
pub async fn save_track(track: Track, server_display: String, map: String, announce: bool) {
    let dir = path::track_dir(&server_display, &map);
    let category = path::sanitize_component(&track.name);

    match save_track_to(&track, &server_display, &map, &dir, &category) {
        Ok(SaveOutcome::Wrote(path)) => {
            info!(?path, "wrote new track version");
            let filename = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            chat_print_async(format!("&aLiveSplit: saved {filename}"));
        }
        Ok(SaveOutcome::AlreadyLatest) => {
            debug!("track unchanged from latest on-disk version; no write");
            if announce {
                chat_print_async("&eLiveSplit: track already saved (no changes)".to_owned());
            }
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
    server: &str,
    map: &str,
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

    let xml = build_lss_xml(track, server, map, &canonical)?;

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

fn same_as_latest(path: &Path, canonical: &str) -> Result<bool> {
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
    // The dedup key is the canonical `LS …` geometry text. Re-canonicalize
    // the stored value (decode -> re-encode) so XML reflow / whitespace
    // and label-only edits (labels aren't in the payload) compare equal.
    // A failure to decode (legacy base64 value, corrupt, or reflowed
    // beyond recovery) falls through to "write a new version" -- the same
    // graceful behavior as a parse failure above.
    match decode::decode_geometry(stored).and_then(|t| payload::serialize_canonical(&t)) {
        Ok(recanonical) => Ok(recanonical == canonical),
        Err(e) => {
            debug!(?path, error = ?e, "could not re-canonicalize stored payload; will write new");
            Ok(false)
        }
    }
}

fn build_lss_xml(track: &Track, server: &str, map: &str, canonical: &str) -> Result<String> {
    let mut run = Run::new();
    run.set_game_name("ClassiCube");

    // Where each piece of track metadata lives in the `.lss`:
    //   - title         -> `<CategoryName>` (bare, color-stripped)
    //   - server + map  -> informational `ClassiCubeServer`/`ClassiCubeMap`
    //                      custom vars (color-stripped, write-only, never
    //                      read back; the file path stays the matching index)
    //   - geometry      -> `ClassiCubeTrack` custom var (read + dedup)
    //   - labels        -> `<Segment>` elements
    //
    // The category name is the bare (color-stripped) track name. On read,
    // `<CategoryName>` is the title source (the payload no longer carries
    // an `LS title` line). It's color-stripped here, so the in-memory title
    // comes back without color codes -- the filename category stem stays
    // stable across a load -> edit -> re-save cycle because
    // `sanitize_component` also strips color first and is idempotent on
    // already-stripped text.
    run.set_category_name(remove_color(&track.name));

    // Server + starting map as self-describing metadata. These give a
    // moved/shared/imported `.lss` a recoverable binding context and let
    // the LiveSplit UI surface the scope. Color-stripped for a clean UI
    // value (matching `<CategoryName>`); livesplit-core XML-escapes the
    // value, so any residual characters round-trip safely. Write-only:
    // never read back, and not part of the dedup key (server/map are
    // invariant per directory, so they can't cause write churn).
    //
    // Known limitation: if a server's *display* name changes but still
    // sanitizes to the same directory slug (e.g. a color-code tweak) and
    // the geometry is unchanged, the dedup gate skips the write and the
    // latest file keeps the first-seen server string. A cosmetic edge case;
    // not worth adding server/map to the dedup key for.
    run.metadata_mut()
        .custom_variable_mut(SERVER_VAR_NAME)
        .permanent()
        .set_value(remove_color(server));
    run.metadata_mut()
        .custom_variable_mut(MAP_VAR_NAME)
        .permanent()
        .set_value(remove_color(map));

    // LiveSplit's segment list is everything after the implicit Start:
    // pressing Start is the timer-side run-start action, not a named
    // split (you run *into* the first split). So the Start checkpoint at
    // index 0 doesn't get a `<Segment>`; the rest do, in order. The
    // reader mirrors this -- it expects one label per non-Start
    // checkpoint and defaults the Start's label.
    for cp in track.checkpoints.iter().skip(1) {
        run.push_segment(Segment::new(cp.label.as_str()));
    }

    // `canonical` is the readable geometry-only `LS …` text (newline-
    // joined bare lines). livesplit-core XML-escapes the value on save
    // and unescapes it on load, so any `&` color codes survive intact.
    // Neither the title nor labels are in this payload -- the title is
    // the `<CategoryName>` above and labels are the `<Segment>` elements.
    run.metadata_mut()
        .custom_variable_mut(CUSTOM_VAR_NAME)
        .permanent()
        .set_value(canonical);

    let mut xml = String::new();
    save_run(&run, &mut xml).map_err(|e| anyhow!("saving Run to XML: {e}"))?;
    Ok(xml)
}
