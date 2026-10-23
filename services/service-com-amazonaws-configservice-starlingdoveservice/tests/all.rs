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
#[path = "all/config_aggregation_authorization.rs"]
mod config_aggregation_authorization;
#[path = "all/config_aggregator_lifecycle.rs"]
mod config_aggregator_lifecycle;
#[path = "all/config_compliance_by_config_rule.rs"]
mod config_compliance_by_config_rule;
#[path = "all/config_connectors.rs"]
mod config_connectors;
#[path = "all/config_delivery_channel_status.rs"]
mod config_delivery_channel_status;
#[path = "all/config_discovered_resource_counts.rs"]
mod config_discovered_resource_counts;
#[path = "all/config_read_shape_and_compliance.rs"]
mod config_read_shape_and_compliance;
#[path = "all/config_remediation_configurations.rs"]
mod config_remediation_configurations;
#[path = "all/config_retention_configuration_lifecycle.rs"]
mod config_retention_configuration_lifecycle;
#[path = "all/config_rule_evaluation_status.rs"]
mod config_rule_evaluation_status;
#[path = "all/config_service_linked_recorder.rs"]
mod config_service_linked_recorder;
#[path = "all/config_setup_and_rule_loop.rs"]
mod config_setup_and_rule_loop;
#[path = "all/config_stored_queries.rs"]
mod config_stored_queries;
#[path = "all/config_tagging.rs"]
mod config_tagging;
#[path = "all/config_third_party_service_linked_recorder.rs"]
mod config_third_party_service_linked_recorder;
#[path = "all/discovered_resources.rs"]
mod discovered_resources;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/list_configuration_recorders.rs"]
mod list_configuration_recorders;
#[path = "all/pending_aggregation_requests.rs"]
mod pending_aggregation_requests;
#[path = "all/resource_evaluations.rs"]
mod resource_evaluations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

// Declared after the sorted list on purpose: every new suite this crate gains is
// inserted into the `config_*` block above, and a declaration added there conflicts
// with the next one that lands. Fold these lines into the sorted list whenever the
// list is next touched for another reason.
#[path = "all/config_custom_rule_policy.rs"]
mod config_custom_rule_policy;
#[path = "all/compliance_details_by_config_rule.rs"]
mod compliance_details_by_config_rule;
#[path = "all/proactive_resource_evaluation.rs"]
mod proactive_resource_evaluation;
#[path = "all/config_deliver_config_snapshot.rs"]
mod config_deliver_config_snapshot;
#[path = "all/aggregate_resource_config.rs"]
mod aggregate_resource_config;
#[path = "all/resource_evaluation_surface.rs"]
mod resource_evaluation_surface;
