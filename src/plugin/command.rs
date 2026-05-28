use std::{cell::RefCell, os::raw::c_int, slice};

use classicube_sys::{OwnedChatCommand, cc_string};
use tracing::debug;

use crate::{
    chat_print,
    plugin::{
        is_plugin_active,
        livesplit::{self, Command as LsCommand},
        module::Module,
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
        ["start"] => livesplit::send(LsCommand::Start),
        ["split"] => livesplit::send(LsCommand::Split),
        ["splitorstart"] => livesplit::send(LsCommand::SplitOrStart),
        ["pause"] => livesplit::send(LsCommand::Pause),
        ["resume"] => livesplit::send(LsCommand::Resume),
        ["reset"] => livesplit::send(LsCommand::Reset { save_attempt: None }),
        ["undosplit"] => livesplit::send(LsCommand::UndoSplit),
        ["skipsplit"] => livesplit::send(LsCommand::SkipSplit),
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
