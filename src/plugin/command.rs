use std::{cell::RefCell, os::raw::c_int, slice};

use classicube_helpers::{chat, tab_list::TabList};
use classicube_sys::{OwnedChatCommand, cc_string};
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        editor, hud, is_plugin_active,
        livesplit::{self, Command as LsCommand, protocol::TimingMethod},
        lss_storage,
        module::Module,
        pause_triggers, splits,
        track_source::encode::encode_for_chat,
    },
};

thread_local!(
    static COMMAND: RefCell<Option<OwnedChatCommand>> = const { RefCell::new(None) };
);

// `arboard::Clipboard` owns a daemon thread on Linux X11 that holds the
// selection until another app takes ownership or the `Clipboard` is dropped.
// Keep one alive across `nas` invocations so the copied text stays pasteable
// after the chat command returns.
thread_local!(
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
);

fn clipboard_set_text(s: String) -> anyhow::Result<()> {
    CLIPBOARD.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(arboard::Clipboard::new()?);
        }
        slot.as_mut().unwrap().set_text(s)?;
        Ok(())
    })
}

fn print_usage() {
    chat_print("&eUsage:");
    chat_print("&e  /client LiveSplit status");
    chat_print("&e  /client LiveSplit start | split | splitorstart");
    chat_print("&e  /client LiveSplit pause | resume | reset");
    chat_print("&e  /client LiveSplit undosplit | skipsplit");
    chat_print("&e  /client LiveSplit loadtest | splits | clear | save");
    chat_print("&e  /client LiveSplit show [on|off]    (toggle checkpoint HUD)");
    chat_print("&e  /client LiveSplit edit on|off");
    chat_print("&e  /client LiveSplit edit place [i] | cancel | select <i>");
    chat_print("&e  /client LiveSplit edit delete [i] | label <i> <text> | clear");
    chat_print("&e  /client LiveSplit track encode");
    chat_print("&e  /client LiveSplit mb <subcmd ...>  (one chained /mb to deliver all lines)");
    chat_print(
        "&e  /client LiveSplit nas               (copies all lines, \\n-separated, to clipboard)",
    );
    chat_print("&e  /client LiveSplit tabdump           (dump tab-list entries for debugging)");
}

/// Parse a checkpoint index argument, chat-printing an error and
/// returning `None` on a non-numeric value.
fn parse_index(s: &str) -> Option<usize> {
    match s.parse::<usize>() {
        Ok(i) => Some(i),
        Err(_) => {
            chat_print(&format!("&cLiveSplit: invalid checkpoint index '{s}'"));
            None
        }
    }
}

fn require_connected() -> bool {
    if livesplit::any_connected() {
        true
    } else {
        chat_print("&cLiveSplit: not connected (run /client LiveSplit status)");
        false
    }
}

/// Encode the currently loaded track for chat delivery, chat-printing
/// the standard error message on `None`/`Err` and returning `Some` only
/// on success. Shared between the `track encode`, `mb`, and `nas` arms.
fn encode_track_or_print_error() -> Option<Vec<String>> {
    match splits::current_track() {
        None => {
            chat_print("&eLiveSplit: no track loaded (try /client LiveSplit loadtest)");
            None
        }
        Some(track) => match encode_for_chat(&track) {
            Ok(lines) => Some(lines),
            Err(e) => {
                chat_print(&format!("&cLiveSplit: encode failed: {e}"));
                None
            }
        },
    }
}

extern "C" fn c_callback(args: *const cc_string, args_count: c_int) {
    // The command list is permanent (no Commands_Unregister), so this
    // function pointer can fire while MAIN_MODULE is None — between a
    // hot-reload Free and the next Init, or permanently after a final
    // Free. Bail before touching any torn-down state.
    if !is_plugin_active() {
        chat_print(
            "&eLiveSplit: plugin not active (between hot-reload Free/Init); ignoring command",
        );
        return;
    }

    let args = unsafe { slice::from_raw_parts(args, args_count as usize) };
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    debug!(?args, "chat command");

    match args.as_slice() {
        ["status"] => {
            chat_print(&format!("&aLiveSplit v{}", env!("CARGO_PKG_VERSION")));
            let server = if livesplit::server_connected() {
                "&aclient connected"
            } else {
                "&7no client"
            };
            chat_print(&format!(
                "&e  server (ws://{}): {server}",
                livesplit::SERVER_BIND_ADDR
            ));
            #[cfg(windows)]
            {
                let client = if livesplit::client_connected() {
                    "&aconnected"
                } else {
                    "&7dialing"
                };
                chat_print(&format!(
                    "&e  client ({}): {client}",
                    livesplit::CLIENT_PIPE_NAME
                ));
            }
        }
        ["start"] => {
            if require_connected() {
                livesplit::send(LsCommand::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                });
                livesplit::send(LsCommand::Start);
                livesplit::send(LsCommand::InitializeGameTime);
            }
        }
        ["split"] => {
            if require_connected() {
                livesplit::send(LsCommand::Split);
            }
        }
        ["splitorstart"] => {
            if require_connected() {
                livesplit::send(LsCommand::SetCurrentTimingMethod {
                    timing_method: TimingMethod::GameTime,
                });
                livesplit::send(LsCommand::SplitOrStart);
                livesplit::send(LsCommand::InitializeGameTime);
            }
        }
        ["pause"] => {
            // Manipulates the local game-time pause counter — same
            // path as `CheckpointKind::Pause` AABB checkpoints and
            // `PauseTriggersModule`'s map-load pause. `pause_add`
            // emits `PauseGameTime` on the 0->1 counter edge; if a
            // timer is connected it pauses game time, if not the
            // counter still tracks so it stacks correctly with
            // subsequent map-load / AABB pauses. Not gated on
            // `require_connected()` because the counter is local
            // plugin state.
            pause_triggers::pause_add();
            chat_print("&aLiveSplit: pause counter +1");
        }
        ["resume"] => {
            // Symmetric inverse of `pause`. `pause_sub` saturates at
            // 0 and emits `ResumeGameTime` on the 1->0 counter edge.
            // If multiple pause sources are stacked (e.g. a Pause
            // AABB plus a manual `pause`), the user may need to call
            // `resume` multiple times to actually unpause.
            pause_triggers::pause_sub();
            chat_print("&aLiveSplit: pause counter -1");
        }
        ["reset"] => {
            splits::reset_run();
            if require_connected() {
                livesplit::send(LsCommand::Reset { save_attempt: None });
            }
        }
        ["undosplit"] => {
            if require_connected() {
                livesplit::send(LsCommand::UndoSplit);
            }
        }
        ["skipsplit"] => {
            if require_connected() {
                livesplit::send(LsCommand::SkipSplit);
            }
        }
        ["loadtest"] => splits::load_fixture(),
        ["splits"] => splits::print_status(),
        ["clear"] => splits::clear_track(),
        ["save"] => lss_storage::save_current_track(),
        ["show"] => {
            let on = hud::toggle_show();
            chat_print(if on {
                "&aLiveSplit: checkpoint HUD on"
            } else {
                "&aLiveSplit: checkpoint HUD off"
            });
        }
        ["show", "on"] => {
            hud::set_show(true);
            chat_print("&aLiveSplit: checkpoint HUD on");
        }
        ["show", "off"] => {
            hud::set_show(false);
            chat_print("&aLiveSplit: checkpoint HUD off");
        }
        // Editor arms -- local plugin/file state, so none gate on
        // `require_connected()` (the editor is usable offline; a
        // mutation mid-run notifies any attached timer via
        // `splits::editor_*`).
        ["edit", "on"] => editor::set_enabled(true),
        ["edit", "off"] => editor::set_enabled(false),
        ["edit", "place"] => editor::arm_place(None),
        ["edit", "place", i] => {
            if let Some(idx) = parse_index(i) {
                editor::arm_place(Some(idx));
            }
        }
        ["edit", "cancel"] => editor::cancel(),
        ["edit", "select", i] => {
            if let Some(idx) = parse_index(i) {
                editor::select(idx);
            }
        }
        ["edit", "delete"] => editor::delete(None),
        ["edit", "delete", i] => {
            if let Some(idx) = parse_index(i) {
                editor::delete(Some(idx));
            }
        }
        ["edit", "label", i, rest @ ..] if !rest.is_empty() => {
            if let Some(idx) = parse_index(i) {
                editor::set_label(idx, rest.join(" "));
            }
        }
        ["edit", "clear"] => editor::clear(),
        ["track", "encode"] => {
            if let Some(lines) = encode_track_or_print_error() {
                let (name, n) = splits::current_track()
                    .map(|t| (t.name, t.checkpoints.len()))
                    .unwrap_or_default();
                let l = lines.len();
                chat_print(&format!(
                    "&aLiveSplit: encoded track (\"{name}\", {n} checkpoints, {l} lines):"
                ));
                for line in &lines {
                    chat_print(line);
                }
            }
        }
        ["mb" | "messageblock", rest @ ..] if !rest.is_empty() => {
            if let Some(lines) = encode_track_or_print_error() {
                let first = &lines[0];
                // MCGalaxy's MessageBlock command-chain separator is `" |/"`
                // (MCGalaxy/MCGalaxy/Blocks/Extended/MessageBlock.cs:85) —
                // space, pipe, slash. The leading space is required: without
                // it the whole chained payload stores as one literal sign
                // message and no subcommand fires on click.
                let chained: String = lines[1..].iter().map(|l| format!(" |/msgme {l}")).collect();
                let payload = format!("/mb {} {first}{chained}", rest.join(" "));
                let cp_len = payload.chars().count();
                chat_print(&format!("&eLiveSplit: sending chained /mb ({cp_len} cp)"));
                chat::send(payload);
            }
        }
        ["tabdump"] => {
            let tab_list = TabList::new();
            let mut entries = tab_list.get_all();
            entries.sort_by_key(|(id, _)| *id);
            chat_print(&format!(
                "&aLiveSplit: tab list ({} entries)",
                entries.len()
            ));
            for (id, weak) in entries {
                let Some(entry) = weak.upgrade() else {
                    continue;
                };
                chat_print(&format!(
                    "&e  [{id}] real={:?} nick={:?} group={:?} rank={}",
                    entry.get_real_name(),
                    entry.get_nick_name(),
                    entry.get_group(),
                    entry.get_rank(),
                ));
            }
        }
        ["nas"] => {
            if let Some(lines) = encode_track_or_print_error() {
                let joined = lines.join("\n");
                let cp_len = joined.chars().count();
                let n = lines.len();
                match clipboard_set_text(joined) {
                    Ok(()) => chat_print(&format!(
                        "&aLiveSplit: copied NAS lines ({n} lines, {cp_len} cp total) to clipboard"
                    )),
                    Err(e) => {
                        chat_print(&format!("&cLiveSplit: clipboard write failed: {e}"));
                    }
                }
            }
        }
        _ => print_usage(),
    }
}

pub struct CommandModule;

impl CommandModule {
    pub fn init() -> Self {
        COMMAND.with(|cell| {
            // Commands_Register appends to a global linked list
            // (cmds_head/cmds_tail) with no Commands_Unregister. Re-registering
            // on hot reload either duplicates the entry or, if the previous
            // OwnedChatCommand was dropped, leaves a dangling pointer (UAF on
            // the next /client …). Register once per process and keep the
            // OwnedChatCommand alive in the thread-local forever.
            if cell.borrow().is_some() {
                chat_print(
                    "&eLiveSplit: /client LiveSplit already registered (skipping re-registration \
                     on hot reload)",
                );
                return;
            }
            let mut cmd = OwnedChatCommand::new(
                "LiveSplit",
                c_callback,
                false,
                vec![
                    "&a/client LiveSplit status",
                    "&a/client LiveSplit start | split | splitorstart",
                    "&a/client LiveSplit pause | resume | reset",
                    "&a/client LiveSplit undosplit | skipsplit",
                    "&a/client LiveSplit loadtest | splits | clear | save",
                    "&a/client LiveSplit show [on|off]",
                    "&a/client LiveSplit edit on|off | place [i] | cancel | select <i>",
                    "&a/client LiveSplit edit delete [i] | label <i> <text> | clear",
                    "&a/client LiveSplit track encode",
                    "&a/client LiveSplit mb <subcmd ...>",
                    "&a/client LiveSplit nas",
                    "&a/client LiveSplit tabdump",
                ],
            );
            cmd.register();
            *cell.borrow_mut() = Some(cmd);
        });
        Self
    }
}

impl Module for CommandModule {
    // The `COMMAND` thread-local is intentionally not dropped: that would
    // free heap memory still referenced by ClassiCube's command list (see
    // init()). The `CLIPBOARD` thread-local is fine to drop — clearing it
    // shuts down arboard's Linux daemon thread so hot reload doesn't leak
    // it across Init cycles.
    fn free(&mut self) {
        CLIPBOARD.with_borrow_mut(|c| *c = None);
    }
}
