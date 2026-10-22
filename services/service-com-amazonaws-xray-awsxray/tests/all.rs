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
#[path = "all/encryption_config.rs"]
mod encryption_config;

#[path = "all/graphs_rules_retrievals_and_insights.rs"]
mod graphs_rules_retrievals_and_insights;

#[path = "all/group_control_plane.rs"]
mod group_control_plane;

#[path = "all/group_identifier_exclusivity.rs"]
mod group_identifier_exclusivity;

#[path = "all/indexing_rules.rs"]
mod indexing_rules;

#[path = "all/insight_summaries.rs"]
mod insight_summaries;


#[path = "all/request_deserialization.rs"]
mod request_deserialization;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/sampling_rule_deletion.rs"]
mod sampling_rule_deletion;

#[path = "all/sampling_rules.rs"]
mod sampling_rules;

#[path = "all/service_graph.rs"]
mod service_graph;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/telemetry_records.rs"]
mod telemetry_records;

#[path = "all/time_series_service_statistics.rs"]
mod time_series_service_statistics;

#[path = "all/trace_data_plane_rules.rs"]
mod trace_data_plane_rules;

#[path = "all/trace_graph.rs"]
mod trace_graph;

#[path = "all/trace_ingest_and_groups.rs"]
mod trace_ingest_and_groups;

#[path = "all/trace_retrieval_lifecycle.rs"]
mod trace_retrieval_lifecycle;

#[path = "all/trace_segment_destination.rs"]
mod trace_segment_destination;

#[path = "all/update_group.rs"]
mod update_group;

#[path = "all/xray_wire.rs"]
mod xray_wire;
