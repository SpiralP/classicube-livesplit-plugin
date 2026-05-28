use std::{cell::RefCell, os::raw::c_int, slice};

use classicube_sys::{OwnedChatCommand, cc_string};
use tracing::debug;

use crate::{
    chat_print,
    plugin::{is_plugin_active, module::Module},
};

thread_local!(
    static COMMAND: RefCell<Option<OwnedChatCommand>> = const { RefCell::new(None) };
);

fn print_usage() {
    chat_print("&eUsage:");
    chat_print("&e  /client LiveSplit status");
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
            chat_print(&format!(
                "&aLiveSplit v{} \u{2014} IPC not yet implemented",
                env!("CARGO_PKG_VERSION")
            ));
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
                vec!["&a/client LiveSplit status"],
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
