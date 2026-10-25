// SPDX-License-Identifier: CC0-1.0

//! MOVED — this file is intentionally empty. It carries no test; it stays declared in
//! `tests/all.rs` only because the crate's layout check refuses an undeclared file under
//! `tests/all/`, and an empty module compiles to nothing.
//!
//! The rule it used to carry — the ELBv2 target group an ECS service binds to is paired
//! with the task-definition revision's `networkMode` ALONE, so the placement the request
//! asks for (a `launchType`, a `capacityProviderStrategy`) neither relaxes nor changes the
//! pairing — now lives in the sibling suite `all/load_balancer_target_groups.rs`, as
//! `the_pairing_holds_whatever_placement_the_request_asks_for`. That suite already owns the
//! harness this rule needs (the ECS and Elastic Load Balancing v2 services over one engine,
//! the target-group and listener fixtures, and the create/refuse assertions), and keeping a
//! second copy of it would let the two drift.
//!
//! The content is gone rather than the file because this fixer's tool set can write a file
//! but not remove one; the next change to touch this directory should delete it.
