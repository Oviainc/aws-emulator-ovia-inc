// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/channel_lifecycle.rs`) — which the layout check refuses. The `#[path]` is
// what points it into `tests/all/`.
#[path = "all/batch_delete.rs"]
mod batch_delete;

#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;

#[path = "all/channel_pipeline_restart.rs"]
mod channel_pipeline_restart;

#[path = "all/channel_schedule.rs"]
mod channel_schedule;

#[path = "all/cloud_watch_alarm_template_groups.rs"]
mod cloud_watch_alarm_template_groups;

#[path = "all/cloud_watch_alarm_template_update.rs"]
mod cloud_watch_alarm_template_update;

#[path = "all/cloud_watch_alarm_templates.rs"]
mod cloud_watch_alarm_templates;

#[path = "all/deleted_resource_mutation.rs"]
mod deleted_resource_mutation;

#[path = "all/error_envelope.rs"]
mod error_envelope;

#[path = "all/event_bridge_rule_template_groups.rs"]
mod event_bridge_rule_template_groups;

#[path = "all/event_bridge_rule_template_listing.rs"]
mod event_bridge_rule_template_listing;

#[path = "all/event_bridge_rule_templates.rs"]
mod event_bridge_rule_templates;

#[path = "all/input_deletion.rs"]
mod input_deletion;

#[path = "all/input_endpoint_release.rs"]
mod input_endpoint_release;

#[path = "all/input_lifecycle.rs"]
mod input_lifecycle;

#[path = "all/input_push_endpoints.rs"]
mod input_push_endpoints;

#[path = "all/input_security_group.rs"]
mod input_security_group;

#[path = "all/input_security_group_listing.rs"]
mod input_security_group_listing;

#[path = "all/input_update.rs"]
mod input_update;

#[path = "all/monitor_deployment.rs"]
mod monitor_deployment;

#[path = "all/multiplex_deletion.rs"]
mod multiplex_deletion;

#[path = "all/multiplex_lifecycle.rs"]
mod multiplex_lifecycle;

#[path = "all/multiplex_program_listing.rs"]
mod multiplex_program_listing;

#[path = "all/multiplex_program_update.rs"]
mod multiplex_program_update;

#[path = "all/multiplex_programs.rs"]
mod multiplex_programs;

#[path = "all/multiplex_start.rs"]
mod multiplex_start;

#[path = "all/multiplex_stop.rs"]
mod multiplex_stop;

#[path = "all/rejection_messages.rs"]
mod rejection_messages;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/sdi_source_lifecycle.rs"]
mod sdi_source_lifecycle;

#[path = "all/sdi_source_tags_and_statuses.rs"]
mod sdi_source_tags_and_statuses;

#[path = "all/signal_map_listing.rs"]
mod signal_map_listing;

#[path = "all/signal_maps.rs"]
mod signal_maps;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/template_deletion.rs"]
mod template_deletion;

#[path = "all/template_updates.rs"]
mod template_updates;

// Not a suite of its own: the cloudwatch alarm template group request helpers
// the group suites share, layered on `wire`. Declared because every file under
// `tests/all/` must be.
#[path = "all/support.rs"]
mod support;

#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;

/// The shared restJson1 harness the suites above drive the service through.
#[path = "all/wire.rs"]
mod wire;
