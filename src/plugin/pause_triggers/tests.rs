use std::sync::Mutex;

use super::*;

// The counter is a thread-local. Cargo nextest runs each test in its
// own process so isolation is free there, but `cargo test` shares
// threads — guard with a mutex and reset state at the top of each
// test.
static SERIALIZE: Mutex<()> = Mutex::new(());

fn fresh() -> std::sync::MutexGuard<'static, ()> {
    let g = SERIALIZE.lock().unwrap_or_else(|p| p.into_inner());
    reset_counter();
    g
}

#[test]
fn add_then_sub_round_trips_to_zero() {
    let _g = fresh();
    pause_add();
    assert_eq!(current_counter(), 1);
    pause_sub();
    assert_eq!(current_counter(), 0);
}

#[test]
fn two_sources_stack() {
    let _g = fresh();
    pause_add(); // source A
    pause_add(); // source B
    assert_eq!(current_counter(), 2);
    pause_sub(); // source A resolves; B still holds
    assert_eq!(current_counter(), 1);
    pause_sub(); // source B resolves; back to 0
    assert_eq!(current_counter(), 0);
}

#[test]
fn sub_saturates_at_zero() {
    let _g = fresh();
    pause_sub();
    pause_sub();
    pause_sub();
    assert_eq!(current_counter(), 0);
}

#[test]
fn clear_all_zeroes_counter() {
    let _g = fresh();
    pause_add();
    pause_add();
    pause_add();
    assert_eq!(current_counter(), 3);
    pause_clear_all();
    assert_eq!(current_counter(), 0);
}

#[test]
fn clear_all_is_idempotent_at_zero() {
    let _g = fresh();
    pause_clear_all();
    pause_clear_all();
    assert_eq!(current_counter(), 0);
}

#[test]
fn redundant_add_keeps_climbing() {
    // No upper-bound clamp: every `pause_add` increments. The 0->1
    // edge is the only one that emits PauseGameTime, but the counter
    // tracks every source so symmetric `sub`s are required.
    let _g = fresh();
    pause_add();
    pause_add();
    pause_add();
    pause_add();
    assert_eq!(current_counter(), 4);
}
