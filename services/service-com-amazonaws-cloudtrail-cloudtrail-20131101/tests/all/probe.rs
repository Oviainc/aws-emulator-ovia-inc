// SPDX-License-Identifier: CC0-1.0

//! EMPTY on purpose: there is no suite here, and nothing in this file compiles into one.
//!
//! It briefly held the `GetQueryResults` target-prefix cases under a name that said
//! nothing about what they cover; they now live in
//! `tests/all/query_results_target_prefix.rs`. The file itself stays, declared in
//! `tests/all.rs` as an empty module because the crate's layout check requires every
//! file under `tests/all/` to be declared, only because the tool this emulator's fixers
//! run with can create and edit a file but cannot delete or move one (see
//! `tests/all/wire.rs` and `tests/all/zz_diagnostic.rs`, kept for the same reason).
