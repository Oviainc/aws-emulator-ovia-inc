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
#[path = "all/account_behavior_graph_singleton.rs"]
mod account_behavior_graph_singleton;
#[path = "all/behavior_graph_lifecycle.rs"]
mod behavior_graph_lifecycle;
#[path = "all/client_error_bodies.rs"]
mod client_error_bodies;
#[path = "all/datasource_package_ingest.rs"]
mod datasource_package_ingest;
#[path = "all/datasource_package_reporting.rs"]
mod datasource_package_reporting;
#[path = "all/investigation_state_transitions.rs"]
mod investigation_state_transitions;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/member_datasource_ingest_history.rs"]
mod member_datasource_ingest_history;
#[path = "all/member_ingest_states_and_administrator_membership.rs"]
mod member_ingest_states_and_administrator_membership;
#[path = "all/member_removal.rs"]
mod member_removal;
#[path = "all/member_verification_outcome.rs"]
mod member_verification_outcome;
#[path = "all/not_found_and_validation_semantics.rs"]
mod not_found_and_validation_semantics;
#[path = "all/refusal_texts.rs"]
mod refusal_texts;
#[path = "all/start_investigation_scope_and_merits.rs"]
mod start_investigation_scope_and_merits;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/tag_and_untag_resource.rs"]
mod tag_and_untag_resource;
