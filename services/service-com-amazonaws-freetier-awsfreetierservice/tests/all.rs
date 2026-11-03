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
#[path = "all/account_plan_window.rs"]
mod account_plan_window;
#[path = "all/activity_language_code.rs"]
mod activity_language_code;
#[path = "all/activity_plane_refusals.rs"]
mod activity_plane_refusals;
#[path = "all/date_time_wire_form.rs"]
mod date_time_wire_form;
#[path = "all/dimension_decode.rs"]
mod dimension_decode;
#[path = "all/free_tier.rs"]
mod free_tier;
#[path = "all/language_code_subtag_rule.rs"]
mod language_code_subtag_rule;
#[path = "all/match_option_cardinality.rs"]
mod match_option_cardinality;
#[path = "all/refused_request_media_type.rs"]
mod refused_request_media_type;
#[path = "all/resource_not_found_status.rs"]
mod resource_not_found_status;
#[path = "all/shared_rules.rs"]
mod shared_rules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stored_position_selection.rs"]
mod stored_position_selection;
#[path = "all/unissued_page_token_offset.rs"]
mod unissued_page_token_offset;
#[path = "all/validation_message_rules.rs"]
mod validation_message_rules;
#[path = "all/wire_contract.rs"]
mod wire_contract;
