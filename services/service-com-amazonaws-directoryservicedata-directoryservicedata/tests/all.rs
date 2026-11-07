// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.

#[path = "all/create_conflict_wording.rs"]
mod create_conflict_wording;

#[path = "all/data_access_gate.rs"]
mod data_access_gate;

#[path = "all/directory_authorization.rs"]
mod directory_authorization;

#[path = "all/group_inventory.rs"]
mod group_inventory;

#[path = "all/group_lifecycle.rs"]
mod group_lifecycle;

#[path = "all/group_search.rs"]
mod group_search;

#[path = "all/groups_for_member.rs"]
mod groups_for_member;

#[path = "all/membership_removal.rs"]
mod membership_removal;

#[path = "all/request_layer_envelopes.rs"]
mod request_layer_envelopes;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/user_lifecycle.rs"]
mod user_lifecycle;

#[path = "all/user_search.rs"]
mod user_search;

#[path = "all/validation_envelope.rs"]
mod validation_envelope;

#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
