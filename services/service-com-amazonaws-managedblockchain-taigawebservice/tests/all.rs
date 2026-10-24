// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/accessor_lifecycle.rs"]
mod accessor_lifecycle;
#[path = "all/delete_member.rs"]
mod delete_member;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/invitations.rs"]
mod invitations;
#[path = "all/invite_principal.rs"]
mod invite_principal;
#[path = "all/list_accessors.rs"]
mod list_accessors;
#[path = "all/list_members.rs"]
mod list_members;
#[path = "all/modelled_error_payloads.rs"]
mod modelled_error_payloads;
#[path = "all/network_and_node_lifecycle.rs"]
mod network_and_node_lifecycle;
#[path = "all/not_found_and_malformed_ids.rs"]
mod not_found_and_malformed_ids;
#[path = "all/proposal_lifecycle.rs"]
mod proposal_lifecycle;
#[path = "all/public_networks_and_membership.rs"]
mod public_networks_and_membership;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_node_log_publishing.rs"]
mod update_node_log_publishing;
#[path = "all/vote_on_proposal.rs"]
mod vote_on_proposal;
