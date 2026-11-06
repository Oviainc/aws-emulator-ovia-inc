// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty, and not declared as a module.
//!
//! An earlier revision of this branch delivered a queued asynchronous invocation from
//! here. The same delivery landed on mainline first, as `state::async_invocation::deliver`,
//! and that is the one driver this crate has: this file was emptied when the two were
//! merged so that no second copy of it exists. Nothing compiles it — `src/state.rs`
//! declares `async_invocation` and not this name.
