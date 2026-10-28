// SPDX-License-Identifier: CC0-1.0

//! MOVED — this file is intentionally empty, so nothing here compiles or runs.
//!
//! Its rows now live in the sibling suite `all/delete_stack_export_in_use.rs`, under the
//! name that says what they pin: a `DeleteStack` blocked by an exported output value another
//! live stack still imports is CANCELLED, so the stack goes back to the healthy status it
//! held rather than ending in `DELETE_FAILED`.
//!
//! The content is gone rather than the file because this fixer's tool set can write a file
//! but not remove one; it stays declared in `tests/all.rs` because the layout check refuses
//! an undeclared file under `tests/all/`, and the next change to touch this directory should
//! delete both lines and the file.
