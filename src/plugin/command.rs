use std::{cell::RefCell, os::raw::c_int, slice};

use classicube_helpers::chat;
use classicube_sys::{Clipboard_SetText, OwnedChatCommand, OwnedString, cc_string};
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        is_plugin_active,
        livesplit::{self, Command as LsCommand},
        module::Module,
        splits,
        track_source::encode::encode_for_chat,
    },
};

thread_local!(
    static COMMAND: RefCell<Option<OwnedChatCommand>> = const { RefCell::new(None) };
);

fn print_usage() {
    chat_print("&eUsage:");
    chat_print("&e  /client LiveSplit status");
    chat_print("&e  /client LiveSplit start | split | splitorstart");
    chat_print("&e  /client LiveSplit pause | resume | reset");
    chat_print("&e  /client LiveSplit undosplit | skipsplit");
    chat_print("&e  /client LiveSplit loadtest | splits");
    chat_print("&e  /client LiveSplit track encode");
    chat_print("&e  /client LiveSplit mb <subcmd ...>  (one chained /mb to deliver all lines)");
    chat_print(
        "&e  /client LiveSplit nas               (copies all lines, \\n-separated, to clipboard)",
    );
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

/// `INPUTWIDGET_MAX_LINES * INPUTWIDGET_LEN = 3 * 64`. ClassiCube's
/// chat input widget caps a single typed message at this many codepoints,
/// which sets the practical upper bound on a chained `/mb` payload the
/// operator can send in one go.
const TYPED_INPUT_CAP: usize = 192;

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
        ["start"] => livesplit::send(LsCommand::Start),
        ["split"] => livesplit::send(LsCommand::Split),
        ["splitorstart"] => livesplit::send(LsCommand::SplitOrStart),
        ["pause"] => livesplit::send(LsCommand::Pause),
        ["resume"] => livesplit::send(LsCommand::Resume),
        ["reset"] => {
            splits::reset_run();
            livesplit::send(LsCommand::Reset { save_attempt: None });
        }
        ["undosplit"] => livesplit::send(LsCommand::UndoSplit),
        ["skipsplit"] => livesplit::send(LsCommand::SkipSplit),
        ["loadtest"] => splits::load_fixture(),
        ["splits"] => splits::print_status(),
        ["track", "encode"] => {
            if let Some(lines) = encode_track_or_print_error() {
                let (name, n) = splits::current_track()
                    .map(|t| (t.name, t.checkpoints.len()))
                    .unwrap_or_default();
                let l = lines.len();
                chat_print(&format!(
                    "&aLiveSplit: encoded track (\"{name}\", {n} checkpoints, {l} lines); paste \
                     each into its own /mb sign block in order:"
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
                if cp_len > TYPED_INPUT_CAP {
                    chat_print(&format!(
                        "&cLiveSplit: chained /mb payload too long ({cp_len}cp); use /client \
                         LiveSplit nas to paste manually"
                    ));
                } else {
                    chat::send(payload);
                }
            }
        }
        ["nas"] => {
            if let Some(lines) = encode_track_or_print_error() {
                let joined = lines.join("\n");
                let cp_len = joined.chars().count();
                let n = lines.len();
                let owned = OwnedString::new(&joined);
                unsafe {
                    Clipboard_SetText(owned.as_cc_string());
                }
                chat_print(&format!(
                    "&aLiveSplit: copied NAS lines ({n} lines, {cp_len} cp total) to clipboard"
                ));
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
                    "&a/client LiveSplit loadtest | splits",
                    "&a/client LiveSplit track encode",
                    "&a/client LiveSplit mb <subcmd ...>",
                    "&a/client LiveSplit nas",
                ],
            );
            cmd.register();
            *cell.borrow_mut() = Some(cmd);
        });
        Self
    }
}

impl Module for CommandModule {
    // No free(): dropping the OwnedChatCommand would free heap memory still
    // referenced by ClassiCube's command list. The thread-local keeps it
    // alive for the lifetime of the process; see init().
}
