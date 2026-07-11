//! 2D screen-space draw pass for the built-in timer overlay.
//!
//! Reimplements `Gfx_Draw2DTexture`/`Gfx_Make2DQuad` (not `CC_API`, so
//! not bound in classicube-sys) following the pattern from
//! `classicube-chat-bubbles-plugin/src/plugin/rendering/context/vertex_buffer.rs`.

use std::{cell::RefCell, ffi::c_void, ptr};

use classicube_sys::{
    Game, Gfx, Gfx_BindTexture, Gfx_DrawVb_IndexedTris, Gfx_LoadMatrix, Gfx_LockDynamicVb,
    Gfx_SetAlphaBlending, Gfx_SetAlphaTest, Gfx_SetDepthTest, Gfx_SetDepthWrite,
    Gfx_SetVertexFormat, Gfx_UnlockDynamicVb, GfxResourceID, Matrix, MatrixType__MATRIX_PROJ,
    MatrixType__MATRIX_VIEW, OwnedScreen, OwnedTexture, PackedCol_Make, Texture,
    VertexFormat__VERTEX_FORMAT_TEXTURED, VertexTextured, screen::Priority,
};
use tracing::debug;

use super::{TIMER_STATE, context, format::format_time, texture};
use crate::plugin::{
    editor, livesplit,
    splits::{
        self,
        geometry::{CheckpointKind, format_splits, row_color_code},
    },
    timer::state::Phase,
};

/// Mirror of `hud/shared.rs:calc_ortho_matrix`, needed here because
/// `hud::shared` is a private module. Keeps the timer overlay independent
/// of the HUD module's internals.
fn calc_ortho_matrix(width: f32, height: f32, z_near: f32, z_far: f32) -> Matrix {
    let mut m = Matrix::IDENTITY;
    m.row1.x = 2.0 / width;
    m.row2.y = -2.0 / height;
    if cfg!(target_os = "windows") {
        let adjust_x = 0.5 * (2.0 / width);
        let adjust_y = 0.5 * (-2.0 / height);
        m.row3.z = 1.0 / (z_near - z_far);
        m.row4.x = -1.0 - adjust_x;
        m.row4.y = 1.0 - adjust_y;
        m.row4.z = z_near / (z_near - z_far);
    } else {
        m.row3.z = -2.0 / (z_far - z_near);
        m.row4.x = -1.0;
        m.row4.y = 1.0;
        m.row4.z = -(z_far + z_near) / (z_far - z_near);
    }
    m
}

const MARGIN: i16 = 8;
const ROW_GAP: i16 = 2;
/// Top margin used in edit mode (left side). Clears the debug HUD text block
/// in the top-left corner: line1 (FPS/verts) + posAtlas (Position:) + line2
/// (hacks), each ~20 px tall starting at y=2, plus `MARGIN` gap.
const EDIT_MARGIN_TOP: i16 = 2 + 3 * 20 + MARGIN;

thread_local! {
    /// Cached texture for the clock line, keyed by the formatted string.
    static CLOCK_TEX: RefCell<Option<(String, OwnedTexture)>> = const { RefCell::new(None) };

    /// Cached textures for the split rows, keyed by the rendered row strings.
    /// Rebuilt whenever any split fires, is undone, the run resets, or the
    /// edit-mode toggle changes the row format.
    static SPLIT_TEXTURES: RefCell<Vec<Option<OwnedTexture>>> = const { RefCell::new(Vec::new()) };
    /// The rendered row strings `SPLIT_TEXTURES` was built from.
    static LAST_SPLIT_KEY: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Speeds for cwake support
    static LOCKED_SPEEDS: RefCell<Vec<u32>> = RefCell::new(Vec::new());
}

pub fn invalidate() {
    CLOCK_TEX.with_borrow_mut(|c| *c = None);
    SPLIT_TEXTURES.with_borrow_mut(Vec::clear);
    LAST_SPLIT_KEY.with_borrow_mut(Vec::clear);
}

/// Build the render hook and register it at `Priority::OverHud` so the
/// timer clock draws above the engine's HUD (hotbar, chat).
pub fn install() -> OwnedScreen {
    let mut screen = OwnedScreen::new();
    screen.on_render(render);
    screen.add(Priority::OverHud);
    screen
}

unsafe extern "C" fn render(_elem: *mut c_void, _delta: f32) {
    unsafe {
        if Gfx.LostContext != 0 {
            return;
        }
        let Some(vb) = context::vb_resource_id() else {
            return;
        };
        // No track loaded: nothing to show.
        // In edit mode the overlay is always shown (authoring aid: mirrors the
        // in-world labels, bypasses the timer on/off toggle). Outside edit mode
        // the usual show-mode + external-timer gate applies.
        let edit_mode = editor::is_enabled();
        let visible = splits::track_loaded()
            && (edit_mode
                || match super::SHOW_MODE.get() {
                    super::ShowMode::Auto => !livesplit::external_connected(),
                    super::ShowMode::On => true,
                    super::ShowMode::Off => false,
                });
        if !visible {
            return;
        }

        // Restore 2D ortho state defensively: the HUD screen at a lower
        // priority may have left us in 3D state. Gfx_Begin2D already did
        // this, but other OwnedScreen hooks can clobber it.
        #[expect(
            clippy::cast_precision_loss,
            reason = "window dimensions are small positive ints"
        )]
        let ortho = calc_ortho_matrix(Game.Width as f32, Game.Height as f32, -100.0, 1000.0);
        Gfx_LoadMatrix(MatrixType__MATRIX_PROJ, &ortho);
        Gfx_LoadMatrix(MatrixType__MATRIX_VIEW, &Matrix::IDENTITY);
        Gfx_SetAlphaBlending(1);
        Gfx_SetDepthTest(0);
        Gfx_SetDepthWrite(0);
        Gfx_SetAlphaTest(0);
        Gfx_SetVertexFormat(VertexFormat__VERTEX_FORMAT_TEXTURED);

        draw_overlay(vb, edit_mode);
    }
}

fn draw_overlay(vb: GfxResourceID, edit_mode: bool) {
    TIMER_STATE.with_borrow(|slot| {
        let Some(state) = slot.as_ref() else {
            return;
        };

        // Right edge of the play area; rows are right-aligned against it so
        // the overlay hugs the right side of the screen (mirrors the
        // left-anchored `MARGIN` used for `y`).
        #[expect(clippy::cast_possible_truncation, reason = "window width fits in i16")]
        let screen_width = unsafe { Game.Width } as i16;

        let clock = super::clock();
        let elapsed = state.elapsed_now(clock);

        // Insert Cwake speeds to row
        if !edit_mode {
            LOCKED_SPEEDS.with_borrow_mut(|speeds| {
                if state.split_rows.is_empty() {
                    speeds.clear();
                } else {
                    if speeds.len() != state.split_rows.len() {
                        speeds.resize(state.split_rows.len(), 0);
                    }
                    // Clear cache values if the run resets back to start
                    if state.split_rows.iter().all(|r| r.time.is_none()) {
                        speeds.fill(0);
                    }
                    for (i, row) in state.split_rows.iter().enumerate() {
                        if row.time.is_some() && speeds[i] == 0 {
                            speeds[i] = crate::get_live_speed();
                        }
                    }
                }
            });
        }
        /* --------------------------------- */

        // --- Clock line (play mode only) ---
        // In edit mode there is no run to time, so the clock is hidden.
        let clock_height = if edit_mode {
            0
        } else {
            let clock_str = format_time(elapsed);
            let clock_color = phase_color(state.phase);

            // Rebuild clock texture if the string changed (happens every frame
            // while Running since the centisecond digits update continuously).
            let needs_clock_rebuild =
                CLOCK_TEX.with_borrow(|slot| slot.as_ref().is_none_or(|(s, _)| s != &clock_str));
            if needs_clock_rebuild {
                let new_tex = texture::create_clock_texture(&clock_str);
                CLOCK_TEX.with_borrow_mut(|slot| {
                    *slot = new_tex.map(|t| (clock_str.clone(), t));
                });
            }

            CLOCK_TEX.with_borrow(|slot| {
                if let Some((_, tex)) = slot.as_ref() {
                    let t = tex.as_texture();
                    let x = screen_width - t.width as i16 - MARGIN;
                    draw_texture_at(vb, tex, x, MARGIN, clock_color);
                    t.height as i16
                } else {
                    0
                }
            })
        };

        // --- Split rows ---
        // Edit mode: full track list in HUD-label style (`1: label (kind)`),
        // matching the in-world floating labels. No times -- authoring isn't a
        // timed attempt.
        //
        // Play mode, pre-run (NotRunning / after Reset): `split_rows` is empty
        // (only populated at `Command::Start`); derive rows from the loaded
        // track with blank times so the list is visible before a run begins.
        //
        // Play mode, running/ended: `split_rows` is the source of truth,
        // carrying captured per-split times.
        //
        // The cache key is the rendered strings themselves, so any change that
        // affects the display text (mode toggle, editor add/remove/relabel,
        // split fire, undo, reset) automatically invalidates without a
        // separate hook.
        let row_texts: Vec<String> = if edit_mode {
            splits::current_track().map_or_else(Vec::new, |track| {
                let mut lines = format_splits(&track, &[], None, false);
                if !lines.is_empty() {
                    lines.remove(0); // drop the header line
                }
                lines
            })
        } else if state.split_rows.is_empty() {
            splits::checkpoint_rows()
                .into_iter()
                .map(|(kind, label, is_map)| play_row_text(kind, &label, None, is_map))
                .collect()
        } else {
            // Append checkpoint speeds to time row
            LOCKED_SPEEDS.with_borrow(|speeds| {
                state
                    .split_rows
                    .iter()
                    .enumerate()
                    .map(|(i, row)| {
                        let mut base_text = play_row_text(row.kind, &row.label, row.time, row.is_map);
                        if let Some(&speed) = speeds.get(i) { if speed > 0 {
                            let speed_float = speed as f32 / 100.0;

                            if speed_float < 10.0 { base_text = format!("{}  &b({:.2})", base_text, speed_float); }
                            else if speed_float < 100.0 { base_text = format!("{}  &b({:.1})", base_text, speed_float); }
                            else { base_text = format!("{}  &b({:.0})", base_text, speed_float); }
                            
                        }
                    } base_text })
                .collect()
            })
        };

        let needs_rebuild =
            LAST_SPLIT_KEY.with_borrow(|last| last.as_slice() != row_texts.as_slice());
        if needs_rebuild {
            debug!("rebuilding timer split row textures");
            let textures: Vec<Option<OwnedTexture>> = row_texts
                .iter()
                .map(|text| texture::create_split_texture(text))
                .collect();
            SPLIT_TEXTURES.with_borrow_mut(|slot| *slot = textures);
            LAST_SPLIT_KEY.with_borrow_mut(|last| *last = row_texts);
        }

        let mut y = if edit_mode {
            EDIT_MARGIN_TOP
        } else {
            MARGIN + clock_height + ROW_GAP
        };
        SPLIT_TEXTURES.with_borrow(|textures| {
            for tex in textures.iter().flatten() {
                let t = tex.as_texture();
                let x = if edit_mode {
                    MARGIN
                } else {
                    screen_width - t.width as i16 - MARGIN
                };
                draw_texture_at(vb, tex, x, y, PackedCol_Make(255, 255, 255, 255));
                y += t.height as i16 + ROW_GAP;
            }
        });
    });
}

/// Format a play-mode split row: kind-colored label and time side by side.
/// Empty label shows just the time. `time = None` renders the placeholder.
/// `is_map` selects the purple map-transition color instead of the kind color.
fn play_row_text(kind: CheckpointKind, label: &str, time: Option<f64>, is_map: bool) -> String {
    let time_str = time.map(format_time);
    let time_str = time_str.as_deref().unwrap_or("--:--.---");
    let code = row_color_code(kind, is_map);
    if label.is_empty() {
        format!("{code}{time_str}")
    } else {
        format!("{code}{label}  &f{time_str}")
    }
}

fn phase_color(phase: Phase) -> u32 {
    match phase {
        Phase::NotRunning => PackedCol_Make(200, 200, 200, 255),
        Phase::Running => PackedCol_Make(0, 255, 80, 255),
        Phase::Ended => PackedCol_Make(255, 200, 0, 255),
    }
}

/// Draw a texture positioned at `(x, y)` screen pixels with the given color.
fn draw_texture_at(vb: GfxResourceID, tex: &OwnedTexture, x: i16, y: i16, color: u32) {
    // SAFETY: The OwnedTexture outlives this stack frame; get_texture returns
    // a value copy via struct spread so the GPU id stays valid.
    let mut t = unsafe { tex.get_texture() };
    t.x = x;
    t.y = y;
    unsafe {
        Gfx_BindTexture(t.ID);
        let verts = make_2d_quad(&t, color);
        let dst = Gfx_LockDynamicVb(vb, VertexFormat__VERTEX_FORMAT_TEXTURED, 4);
        ptr::copy_nonoverlapping(verts.as_ptr(), dst.cast::<VertexTextured>(), 4);
        Gfx_UnlockDynamicVb(vb);
        Gfx_DrawVb_IndexedTris(4);
    }
}

/// Build four counter-clockwise `VertexTextured` verts from a positioned
/// `Texture`. Mirrors `Gfx_Draw2DTexture` / `Gfx_Make2DQuad` from
/// `classicube-chat-bubbles-plugin/src/plugin/rendering/context/vertex_buffer.rs`.
fn make_2d_quad(tex: &Texture, color: u32) -> [VertexTextured; 4] {
    let x1 = f32::from(tex.x);
    let x2 = x1 + f32::from(tex.width);
    let y1 = f32::from(tex.y);
    let y2 = y1 + f32::from(tex.height);
    [
        VertexTextured {
            x: x1,
            y: y1,
            z: 0.0,
            Col: color,
            U: tex.uv.u1,
            V: tex.uv.v1,
        },
        VertexTextured {
            x: x2,
            y: y1,
            z: 0.0,
            Col: color,
            U: tex.uv.u2,
            V: tex.uv.v1,
        },
        VertexTextured {
            x: x2,
            y: y2,
            z: 0.0,
            Col: color,
            U: tex.uv.u2,
            V: tex.uv.v2,
        },
        VertexTextured {
            x: x1,
            y: y2,
            z: 0.0,
            Col: color,
            U: tex.uv.u1,
            V: tex.uv.v2,
        },
    ]
}
