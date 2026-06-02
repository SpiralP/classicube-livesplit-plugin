pub mod async_manager;
pub mod command;
pub mod editor;
pub mod hud;
pub mod livesplit;
pub mod logger;
pub mod lss_storage;
pub mod module;
pub mod pause_triggers;
pub mod splits;
pub mod track_source;

use std::cell::RefCell;

use crate::plugin::{
    async_manager::AsyncManagerModule, command::CommandModule, editor::EditorModule,
    hud::HudModule, livesplit::LiveSplitModule, logger::LoggerModule,
    lss_storage::LssStorageModule, module::Module, pause_triggers::PauseTriggersModule,
    splits::SplitsModule, track_source::TrackSourceModule,
};

thread_local!(
    static MAIN_MODULE: RefCell<Option<MainModule>> = const { RefCell::new(None) };
);

struct MainModule {
    logger: LoggerModule,
    async_manager: AsyncManagerModule,
    livesplit: LiveSplitModule,
    splits: SplitsModule,
    pause_triggers: PauseTriggersModule,
    track_source: TrackSourceModule,
    lss_storage: LssStorageModule,
    hud: HudModule,
    editor: EditorModule,
    command: CommandModule,
}

impl MainModule {
    fn init() -> Self {
        let logger = LoggerModule::init();
        let async_manager = AsyncManagerModule::init();
        let livesplit = LiveSplitModule::init();
        let splits = SplitsModule::init();
        let pause_triggers = PauseTriggersModule::init();
        let track_source = TrackSourceModule::init();
        let lss_storage = LssStorageModule::init();
        let hud = HudModule::init();
        let editor = EditorModule::init();
        let command = CommandModule::init();

        Self {
            logger,
            async_manager,
            livesplit,
            splits,
            pause_triggers,
            track_source,
            lss_storage,
            hud,
            editor,
            command,
        }
    }
}

impl Module for MainModule {
    fn children(&mut self) -> Vec<&mut dyn Module> {
        // `pause_triggers` sits **after** `splits` so reverse-dispatch
        // (newest-first) fires `Command::ResumeGameTime` from
        // `pause_triggers.on_new_map_loaded` *before*
        // `splits.on_new_map_loaded` can emit `Command::Split` from a
        // `MapLoaded` checkpoint — the split must land on a resumed
        // timer, not a paused one. Symmetrically, `on_new_map` fires
        // `PauseGameTime` before any splits-side reaction.
        //
        // `lss_storage` sits **after** `track_source` so its `free()`
        // (which clears the splits load-callback slot) runs *before*
        // `track_source.free()` and `splits.free()` in reverse
        // dispatch — `splits` must still have a live callback target
        // through any pre-teardown chat broadcast that triggers
        // `load_track`. `lss_storage` has no `on_new_map_loaded`
        // hook itself; its autoload is tick-driven (see module doc)
        // so it sees the *settled* map name rather than the stale one
        // available at event time on multiplayer.
        //
        // `hud` sits **after** `splits` (its tick reconcile reads
        // `splits::current_track()`, so splits must still be live) and
        // **before** `command` (the `show …` chat arms call into hud
        // accessors). Reverse-dispatch then tears `hud` down before
        // `splits`, clearing the in-world selection boxes while the
        // track snapshot it mirrors is still available.
        //
        // `editor` sits **after** `splits`/`hud` (its commit path mutates
        // splits state and the boxes become visible through hud) and
        // **before** `command` (the `edit …` arms dispatch into editor
        // fns). Reverse-dispatch tears `command` down first, then
        // `editor` (uninstalling the `Server.SendBlock` hook), then
        // `hud`/`splits`. Its `on_new_map_loaded` re-asserts the hook,
        // ordered after splits/hud have settled the map change.
        vec![
            &mut self.logger,
            &mut self.async_manager,
            &mut self.livesplit,
            &mut self.splits,
            &mut self.pause_triggers,
            &mut self.track_source,
            &mut self.lss_storage,
            &mut self.hud,
            &mut self.editor,
            &mut self.command,
        ]
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
