// SPDX-License-Identifier: CC0-1.0

//! An EMPTY suite: the wording of the generated-template concurrency refusal is NOT settled.
//!
//! This file held a table pinning that refusal's whole message. It asserts nothing now,
//! because the only wording available to assert was this crate's own: the message AWS answers
//! for `ConcurrentResourcesLimitExceededException` is not among the recordings this emulator
//! has, and a test that re-states the literal the handler carries cannot fail against the
//! wrong wording. The cause is left open on its own issue, which is where the recorded
//! sentence has to come from; the refusal's STATUS and modelled error code are covered by
//! `generated_template_refusals.rs`, which asserts them from the model.
//!
//! Declared in `tests/all.rs` because the layout check refuses an undeclared file under
//! `tests/all/`, and this fixer's tool set can write a file but not remove one; the change
//! that words this refusal from a recording should take both lines and this file with it.
