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
#[path = "all/airflow_rest_api.rs"]
mod airflow_rest_api;
#[path = "all/env_endpoint_front_door.rs"]
mod env_endpoint_front_door;
#[path = "all/env_endpoint_not_found_wording.rs"]
mod env_endpoint_not_found_wording;
#[path = "all/env_endpoint_payload.rs"]
mod env_endpoint_payload;
#[path = "all/environment_business_rules.rs"]
mod environment_business_rules;
#[path = "all/environment_deletion_teardown.rs"]
mod environment_deletion_teardown;
#[path = "all/environment_endpoint_management_lifecycle.rs"]
mod environment_endpoint_management_lifecycle;
#[path = "all/environment_lifecycle.rs"]
mod environment_lifecycle;
#[path = "all/environment_region_scoping.rs"]
mod environment_region_scoping;
#[path = "all/environment_service_defaults.rs"]
mod environment_service_defaults;
#[path = "all/environment_teardown_write_gate.rs"]
mod environment_teardown_write_gate;
#[path = "all/environment_unit_count_ranges.rs"]
mod environment_unit_count_ranges;
#[path = "all/environment_update_apply_phase.rs"]
mod environment_update_apply_phase;
#[path = "all/environment_update_status_sequence.rs"]
mod environment_update_status_sequence;
#[path = "all/list_environments_cursor_scope.rs"]
mod list_environments_cursor_scope;
#[path = "all/rejection_contract.rs"]
mod rejection_contract;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tags_member_single_rule.rs"]
mod tags_member_single_rule;
