#[cfg(test)]
mod tests;

use std::cell::Cell;

use tracing::debug;

use crate::plugin::{
    livesplit::{self, Command},
    module::Module,
    splits,
};

thread_local! {
    /// Number of active pause sources. Game time stays paused while
    /// the counter is non-zero. The first source to increment from 0
    /// emits `Command::PauseGameTime`; the last source to decrement
    /// to 0 emits `Command::ResumeGameTime`. Lives outside
    /// `PauseTriggersModule` so other modules' tick code can call
    /// `pause_add` / `pause_sub` without going through `MAIN_MODULE`
    /// (which is mid-borrow during dispatch).
    static COUNTER: Cell<u32> = const { Cell::new(0) };
}

/// Bump the pause counter and emit `PauseGameTime` on the 0->1 edge.
pub fn pause_add() {
    let next = COUNTER.get().saturating_add(1);
    COUNTER.set(next);
    if next == 1 {
        debug!("pause counter 0->1; emitting PauseGameTime");
        livesplit::send(Command::PauseGameTime);
    }
}

/// Drop the pause counter (saturating at 0) and emit `ResumeGameTime`
/// on the 1->0 edge.
pub fn pause_sub() {
    let cur = COUNTER.get();
    if cur == 0 {
        return;
    }
    let next = cur - 1;
    COUNTER.set(next);
    if next == 0 {
        debug!("pause counter 1->0; emitting ResumeGameTime");
        livesplit::send(Command::ResumeGameTime);
    }
}

/// Force the counter to 0 and emit a single `ResumeGameTime` if it was
/// non-zero. Used by `SplitsState::rearm()` (Reset) so a fresh attempt
/// can't inherit a stuck pause from a previous abandoned run. Chat
/// commands manipulate the counter symmetrically via `pause_add` /
/// `pause_sub` instead.
pub fn pause_clear_all() {
    if COUNTER.replace(0) > 0 {
        debug!("pause counter cleared; emitting ResumeGameTime");
        livesplit::send(Command::ResumeGameTime);
    }
}

#[cfg(test)]
pub(crate) fn current_counter() -> u32 {
    COUNTER.get()
}

#[cfg(test)]
pub(crate) fn reset_counter() {
    COUNTER.set(0);
}

pub struct PauseTriggersModule;

impl PauseTriggersModule {
    pub fn init() -> Self {
        Self
    }
}

impl Module for PauseTriggersModule {
    fn free(&mut self) {
        COUNTER.set(0);
    }

    fn on_new_map(&mut self) {
        // Without a loaded track there's no run to game-time, and LSO
        // would respond `NoRunInProgress` which surfaces as a chat error.
        if !splits::track_loaded() {
            return;
        }
        debug!("map loading; bumping pause counter");
        pause_add();
    }

    fn on_new_map_loaded(&mut self) {
        if !splits::track_loaded() {
            return;
        }
        debug!("map loaded; dropping pause counter");
        pause_sub();
    }
}
