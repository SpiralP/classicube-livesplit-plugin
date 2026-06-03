use super::*;

// --- edit mode on (index + suffix shown) ---

#[test]
fn display_label_edit_labeled_non_next() {
    assert_eq!(
        display_label(CheckpointKind::Split, 1, "jump-pad", false, true),
        "&e1: jump-pad &e(split)"
    );
}

#[test]
fn display_label_edit_empty_non_next_shows_index_and_kind() {
    assert_eq!(
        display_label(CheckpointKind::Start, 0, "", false, true),
        "&a0: (start)"
    );
}

#[test]
fn display_label_edit_labeled_next_prefixes_marker() {
    assert_eq!(
        display_label(CheckpointKind::End, 3, "final", true, true),
        "&e> &c3: final &c(end)"
    );
}

#[test]
fn display_label_edit_empty_next() {
    assert_eq!(
        display_label(CheckpointKind::Pause, 2, "", true, true),
        "&e> &b2: (pause)"
    );
}

#[test]
fn display_label_edit_resume_labeled() {
    assert_eq!(
        display_label(CheckpointKind::Resume, 4, "x", false, true),
        "&64: x &6(resume)"
    );
}

// --- edit mode off (raw label, no index, no suffix) ---

#[test]
fn display_label_play_labeled_non_next() {
    assert_eq!(
        display_label(CheckpointKind::Split, 1, "jump-pad", false, false),
        "&ejump-pad"
    );
}

#[test]
fn display_label_play_labeled_next_prefixes_marker() {
    assert_eq!(
        display_label(CheckpointKind::End, 3, "final", true, false),
        "&e> &cfinal"
    );
}

#[test]
fn display_label_play_empty_non_next_is_empty() {
    // No label, no kind suffix -- empty body, nothing to draw.
    assert_eq!(
        display_label(CheckpointKind::Start, 0, "", false, false),
        ""
    );
}

#[test]
fn display_label_play_empty_next_is_empty() {
    // is_next marker is suppressed when the body is empty.
    assert_eq!(display_label(CheckpointKind::Pause, 2, "", true, false), "");
}
