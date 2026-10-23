// SPDX-License-Identifier: CC0-1.0

//! EMPTY on purpose: there is no suite here, and nothing in this file compiles into one.
//!
//! It briefly held a wire harness for the channel suite. This crate already has one —
//! `tests/all/emulator.rs`, which every suite shares — so the harness was folded back
//! into it rather than kept twice. The file itself stays, declared in `tests/all.rs` as
//! an empty module because the crate's layout check requires every file under
//! `tests/all/` to be declared, only because the tool this emulator's fixers run with
//! can create and edit a file but cannot delete or move one (see
//! `tests/all/zz_diagnostic.rs`, kept for the same reason).
