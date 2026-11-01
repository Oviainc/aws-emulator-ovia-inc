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
#[path = "all/alias_deletion.rs"]
mod alias_deletion;
#[path = "all/alias_update_isolation.rs"]
mod alias_update_isolation;
#[path = "all/aliases.rs"]
mod aliases;
#[path = "all/builds.rs"]
mod builds;
#[path = "all/error_message_templates.rs"]
mod error_message_templates;
#[path = "all/fleet_attributes.rs"]
mod fleet_attributes;
#[path = "all/game_session_placements.rs"]
mod game_session_placements;
#[path = "all/game_session_queue_updates.rs"]
mod game_session_queue_updates;
#[path = "all/game_session_queues.rs"]
mod game_session_queues;
#[path = "all/hosting_loop.rs"]
mod hosting_loop;
#[path = "all/locations.rs"]
mod locations;
#[path = "all/matchmaking.rs"]
mod matchmaking;
#[path = "all/matchmaking_backfill_mode.rs"]
mod matchmaking_backfill_mode;
#[path = "all/matchmaking_rule_set_persistence.rs"]
mod matchmaking_rule_set_persistence;
#[path = "all/refusal_message_wording.rs"]
mod refusal_message_wording;
#[path = "all/resource_updates.rs"]
mod resource_updates;
#[path = "all/script_content_revisions.rs"]
mod script_content_revisions;
#[path = "all/script_deletion.rs"]
mod script_deletion;
#[path = "all/scripts.rs"]
mod scripts;
#[path = "all/server_process_capacity.rs"]
mod server_process_capacity;
#[path = "all/service_owned_upload_locations.rs"]
mod service_owned_upload_locations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_resource_lookups.rs"]
mod unknown_resource_lookups;
#[path = "all/vpc_peering_authorizations.rs"]
mod vpc_peering_authorizations;
