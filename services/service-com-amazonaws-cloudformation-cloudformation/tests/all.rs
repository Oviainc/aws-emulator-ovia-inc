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
#[path = "all/account_limits.rs"]
mod account_limits;
#[path = "all/agentcore_stack_outputs.rs"]
mod agentcore_stack_outputs;
#[path = "all/appconfig_resource_types.rs"]
mod appconfig_resource_types;
#[path = "all/bucket_notification_configuration.rs"]
mod bucket_notification_configuration;
#[path = "all/callback_signature.rs"]
mod callback_signature;
#[path = "all/change_set_hooks.rs"]
mod change_set_hooks;
#[path = "all/change_set_lifecycle.rs"]
mod change_set_lifecycle;
#[path = "all/change_set_preview.rs"]
mod change_set_preview;
#[path = "all/client_request_tokens.rs"]
mod client_request_tokens;
#[path = "all/cognito_resource_types.rs"]
mod cognito_resource_types;
#[path = "all/content_delivery_stack_outputs.rs"]
mod content_delivery_stack_outputs;
#[path = "all/cross_stack_imports.rs"]
mod cross_stack_imports;
#[path = "all/delete_canceled_export_in_use.rs"]
mod delete_canceled_export_in_use;
#[path = "all/detect_resource_drift.rs"]
mod detect_resource_drift;
#[path = "all/drift_and_resource_reads.rs"]
mod drift_and_resource_reads;
#[path = "all/dynamic_references.rs"]
mod dynamic_references;
#[path = "all/ec2_network_identifiers_feed_rds.rs"]
mod ec2_network_identifiers_feed_rds;
#[path = "all/ecs_elbv2_stack_identifiers.rs"]
mod ecs_elbv2_stack_identifiers;
// An EMPTY, moved-out suite: its rows now live in `all/nested_resource_outputs.rs` and
// `all/provisioned_resources.rs`. Declared because the layout check refuses an undeclared
// file under `tests/all/`, and this fixer's tool set can write a file but not remove one;
// the next change to touch this directory should delete both lines and the file.
#[path = "all/ecs_elbv2_stack_outputs.rs"]
mod ecs_elbv2_stack_outputs;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/estimate_template_cost.rs"]
mod estimate_template_cost;
#[path = "all/extension_registry.rs"]
mod extension_registry;
#[path = "all/failed_create_rollback.rs"]
mod failed_create_rollback;
#[path = "all/generated_physical_names.rs"]
mod generated_physical_names;
#[path = "all/generated_template_body.rs"]
mod generated_template_body;
#[path = "all/generated_template_coverage.rs"]
mod generated_template_coverage;
#[path = "all/generated_template_deletion.rs"]
mod generated_template_deletion;
#[path = "all/generated_template_reads.rs"]
mod generated_template_reads;
#[path = "all/generated_templates.rs"]
mod generated_templates;
#[path = "all/handler_progress_reports.rs"]
mod handler_progress_reports;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/hook_result_detail.rs"]
mod hook_result_detail;
#[path = "all/hook_result_invocations.rs"]
mod hook_result_invocations;
#[path = "all/iam_capability_gate.rs"]
mod iam_capability_gate;
#[path = "all/include_transform.rs"]
mod include_transform;
#[path = "all/list_change_sets.rs"]
mod list_change_sets;
#[path = "all/list_generated_templates.rs"]
mod list_generated_templates;
#[path = "all/list_stack_instances.rs"]
mod list_stack_instances;
#[path = "all/list_stack_resources.rs"]
mod list_stack_resources;
#[path = "all/list_stack_set_auto_deployment_targets.rs"]
mod list_stack_set_auto_deployment_targets;
#[path = "all/list_stack_set_operations.rs"]
mod list_stack_set_operations;
#[path = "all/list_types.rs"]
mod list_types;
#[path = "all/nested_resource_outputs.rs"]
mod nested_resource_outputs;
#[path = "all/no_echo_parameters.rs"]
mod no_echo_parameters;
#[path = "all/provisioned_resources.rs"]
mod provisioned_resources;
#[path = "all/publish_type.rs"]
mod publish_type;
#[path = "all/published_type_schema.rs"]
mod published_type_schema;
#[path = "all/publisher_reads.rs"]
mod publisher_reads;
#[path = "all/publisher_registration.rs"]
mod publisher_registration;
#[path = "all/rds_stack_resources.rs"]
mod rds_stack_resources;
#[path = "all/refused_stack_deletion.rs"]
mod refused_stack_deletion;
#[path = "all/regional_stack_namespace.rs"]
mod regional_stack_namespace;
#[path = "all/resource_scan_lifecycle.rs"]
mod resource_scan_lifecycle;
#[path = "all/resource_scan_related_resources.rs"]
mod resource_scan_related_resources;
#[path = "all/resource_scans.rs"]
mod resource_scans;
#[path = "all/resource_signals.rs"]
mod resource_signals;
#[path = "all/rolled_back_resource_records.rs"]
mod rolled_back_resource_records;
#[path = "all/s3vectors_resource_types.rs"]
mod s3vectors_resource_types;
#[path = "all/resolved_properties.rs"]
mod resolved_properties;
#[path = "all/sam_deployed_resources.rs"]
mod sam_deployed_resources;
#[path = "all/sam_transform.rs"]
mod sam_transform;
#[path = "all/scratch.rs"]
mod scratch;
#[path = "all/stack_delete_failure_reason.rs"]
mod stack_delete_failure_reason;
#[path = "all/stack_event_timeline.rs"]
mod stack_event_timeline;
#[path = "all/stack_exports.rs"]
mod stack_exports;
#[path = "all/stack_failure_reason.rs"]
mod stack_failure_reason;
#[path = "all/stack_lifecycle.rs"]
mod stack_lifecycle;
#[path = "all/stack_output_resolution.rs"]
mod stack_output_resolution;
#[path = "all/stack_own_subnets_db_subnet_group.rs"]
mod stack_own_subnets_db_subnet_group;
#[path = "all/stack_outputs.rs"]
mod stack_outputs;
#[path = "all/stack_reported_members.rs"]
mod stack_reported_members;
#[path = "all/stack_resource_detail.rs"]
mod stack_resource_detail;
#[path = "all/stack_resource_drifts.rs"]
mod stack_resource_drifts;
#[path = "all/stack_resource_events.rs"]
mod stack_resource_events;
#[path = "all/stack_resource_providers.rs"]
mod stack_resource_providers;
#[path = "all/stack_resource_provisioning.rs"]
mod stack_resource_provisioning;
#[path = "all/stack_resources.rs"]
mod stack_resources;
#[path = "all/stack_role_trust.rs"]
mod stack_role_trust;
#[path = "all/stack_set_drift_lifecycle.rs"]
mod stack_set_drift_lifecycle;
#[path = "all/stack_set_lifecycle.rs"]
mod stack_set_lifecycle;
#[path = "all/stack_status_accepts_an_update.rs"]
mod stack_status_accepts_an_update;
#[path = "all/stack_tag_propagation.rs"]
mod stack_tag_propagation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/template_outputs.rs"]
mod template_outputs;
#[path = "all/template_quotas.rs"]
mod template_quotas;
#[path = "all/template_stages_and_resource_events.rs"]
mod template_stages_and_resource_events;
#[path = "all/template_summary.rs"]
mod template_summary;
#[path = "all/template_validation.rs"]
mod template_validation;
#[path = "all/type_deactivation.rs"]
mod type_deactivation;
#[path = "all/type_testing.rs"]
mod type_testing;
#[path = "all/unprovisioned_resources.rs"]
mod unprovisioned_resources;
#[path = "all/unrecognized_resource_types.rs"]
mod unrecognized_resource_types;
#[path = "all/update_rollback_recovery.rs"]
mod update_rollback_recovery;
#[path = "all/validate_template.rs"]
mod validate_template;
#[path = "all/waf_regional_ip_set_resources.rs"]
mod waf_regional_ip_set_resources;
#[path = "all/wait_conditions.rs"]
mod wait_conditions;
