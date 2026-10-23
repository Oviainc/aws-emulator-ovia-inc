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
#[path = "all/aliases.rs"]
mod aliases;
#[path = "all/fleet_attributes.rs"]
mod fleet_attributes;
#[path = "all/game_session_placements.rs"]
mod game_session_placements;
#[path = "all/game_session_queues.rs"]
mod game_session_queues;
#[path = "all/hosting_loop.rs"]
mod hosting_loop;
#[path = "all/locations.rs"]
mod locations;
#[path = "all/matchmaking.rs"]
mod matchmaking;
#[path = "all/matchmaking_rule_set_persistence.rs"]
mod matchmaking_rule_set_persistence;
#[path = "all/server_process_capacity.rs"]
mod server_process_capacity;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_resource_lookups.rs"]
mod unknown_resource_lookups;
