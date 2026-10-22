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
#[path = "all/config_aggregator_lifecycle.rs"]
mod config_aggregator_lifecycle;
#[path = "all/config_compliance_by_config_rule.rs"]
mod config_compliance_by_config_rule;
#[path = "all/config_delivery_channel_status.rs"]
mod config_delivery_channel_status;
#[path = "all/config_read_shape_and_compliance.rs"]
mod config_read_shape_and_compliance;
#[path = "all/config_retention_configuration_lifecycle.rs"]
mod config_retention_configuration_lifecycle;
#[path = "all/config_setup_and_rule_loop.rs"]
mod config_setup_and_rule_loop;
#[path = "all/discovered_resources.rs"]
mod discovered_resources;
#[path = "all/list_configuration_recorders.rs"]
mod list_configuration_recorders;
#[path = "all/pending_aggregation_requests.rs"]
mod pending_aggregation_requests;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
