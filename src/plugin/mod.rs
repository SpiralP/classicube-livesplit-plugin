pub mod async_manager;
pub mod command;
pub mod logger;
pub mod module;

use std::cell::RefCell;

use crate::plugin::{
    async_manager::AsyncManagerModule, command::CommandModule, logger::LoggerModule, module::Module,
};

thread_local!(
    static MAIN_MODULE: RefCell<Option<MainModule>> = const { RefCell::new(None) };
);

struct MainModule {
    logger: LoggerModule,
    async_manager: AsyncManagerModule,
    command: CommandModule,
}

impl MainModule {
    fn init() -> Self {
        let logger = LoggerModule::init();
        let async_manager = AsyncManagerModule::init();
        let command = CommandModule::init();

        Self {
            logger,
            async_manager,
            command,
        }
    }
}

impl Module for MainModule {
    fn children(&mut self) -> Vec<&mut dyn Module> {
        vec![&mut self.logger, &mut self.async_manager, &mut self.command]
    }
}

pub fn is_plugin_active() -> bool {
    MAIN_MODULE.with_borrow(|m| m.is_some())
}

pub fn initialize() {
    MAIN_MODULE.with_borrow_mut(|main_module| {
        if main_module.is_none() {
            *main_module = Some(MainModule::init());
        }
    });
}

pub fn free() {
    MAIN_MODULE.with_borrow_mut(|main_module| {
        if let Some(mut main_module) = main_module.take() {
            main_module.handle_free();
        }
    });
}

pub fn reset() {
    MAIN_MODULE.with_borrow_mut(|main_module| {
        if let Some(main_module) = main_module.as_mut() {
            main_module.handle_reset();
        }
    });
}

pub fn on_new_map() {
    MAIN_MODULE.with_borrow_mut(|main_module| {
        if let Some(main_module) = main_module.as_mut() {
            main_module.handle_on_new_map();
        }
    });
}

pub fn on_new_map_loaded() {
    MAIN_MODULE.with_borrow_mut(|main_module| {
        if let Some(main_module) = main_module.as_mut() {
            main_module.handle_on_new_map_loaded();
        }
    });
}
