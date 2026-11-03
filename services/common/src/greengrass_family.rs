// SPDX-License-Identifier: CC0-1.0

//! Empty, and not declared as a module anywhere: no claim rule lives in this file.
//!
//! The rule for the two emulated services that sign as `greengrass` is
//! [`crate::greengrass`], which landed on mainline while this file was being written and
//! states the same rule at finer grain — the shared routes are matched on their METHOD as
//! well as their URI shape, and the tagging space is split by the ARN's own resource form.
//! Nothing should be added here; call that module.
//!
//! The file is left in place because the issue-fixer tooling can write a file but cannot
//! remove one; nothing compiles it (no module declares it), and it may be deleted outright
//! by anyone touching this crate. Same note as `route_claim.rs`, `service_claim.rs` and
//! `modeled_route_claim.rs`.
