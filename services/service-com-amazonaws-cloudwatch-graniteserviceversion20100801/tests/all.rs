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
#[path = "all/actions_disabled_evaluation_delivery.rs"]
mod actions_disabled_evaluation_delivery;
#[path = "all/alarm_action_dispatch.rs"]
mod alarm_action_dispatch;
#[path = "all/alarm_action_failures.rs"]
mod alarm_action_failures;
#[path = "all/alarm_action_region.rs"]
mod alarm_action_region;
#[path = "all/alarm_actions_switch.rs"]
mod alarm_actions_switch;
#[path = "all/alarm_configuration.rs"]
mod alarm_configuration;
#[path = "all/alarm_delete_semantics.rs"]
mod alarm_delete_semantics;
#[path = "all/alarm_document_timestamps.rs"]
mod alarm_document_timestamps;
#[path = "all/alarm_evaluation.rs"]
mod alarm_evaluation;
#[path = "all/alarm_history.rs"]
mod alarm_history;
#[path = "all/alarm_history_data.rs"]
mod alarm_history_data;
#[path = "all/alarm_m_of_n_directions.rs"]
mod alarm_m_of_n_directions;
#[path = "all/alarm_period_windows.rs"]
mod alarm_period_windows;
#[path = "all/alarm_reason_datapoints.rs"]
mod alarm_reason_datapoints;
#[path = "all/alarm_rule_parse_diagnosis.rs"]
mod alarm_rule_parse_diagnosis;
#[path = "all/anomaly_band_alarms.rs"]
mod anomaly_band_alarms;
#[path = "all/anomaly_detectors.rs"]
mod anomaly_detectors;
#[path = "all/composite_alarms.rs"]
mod composite_alarms;
#[path = "all/dashboard_body_validation.rs"]
mod dashboard_body_validation;
#[path = "all/dashboards.rs"]
mod dashboards;
#[path = "all/datasets.rs"]
mod datasets;
#[path = "all/describe_alarms_for_metric_views.rs"]
mod describe_alarms_for_metric_views;
#[path = "all/disable_alarm_actions.rs"]
mod disable_alarm_actions;
#[path = "all/ec2_alarm_actions.rs"]
mod ec2_alarm_actions;
#[path = "all/evaluation_budget_progress.rs"]
mod evaluation_budget_progress;
#[path = "all/forced_state_actions.rs"]
mod forced_state_actions;
#[path = "all/forced_state_lifetime.rs"]
mod forced_state_lifetime;
#[path = "all/forced_state_reclaim.rs"]
mod forced_state_reclaim;
#[path = "all/get_dashboard_reader.rs"]
mod get_dashboard_reader;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/insight_rule_state.rs"]
mod insight_rule_state;
#[path = "all/insight_rules.rs"]
mod insight_rules;
#[path = "all/list_metric_streams.rs"]
mod list_metric_streams;
#[path = "all/list_tags_for_resource_reads.rs"]
mod list_tags_for_resource_reads;
#[path = "all/log_alarms.rs"]
mod log_alarms;
#[path = "all/managed_insight_rule_catalog.rs"]
mod managed_insight_rule_catalog;
#[path = "all/managed_insight_rules.rs"]
mod managed_insight_rules;
#[path = "all/metric_data.rs"]
mod metric_data;
#[path = "all/metric_math.rs"]
mod metric_math;
#[path = "all/metric_math_alarms.rs"]
mod metric_math_alarms;
#[path = "all/metric_read_identity.rs"]
mod metric_read_identity;
#[path = "all/metric_storage_resolution.rs"]
mod metric_storage_resolution;
#[path = "all/metric_stream_additional_statistics.rs"]
mod metric_stream_additional_statistics;
#[path = "all/metric_stream_scope.rs"]
mod metric_stream_scope;
#[path = "all/metric_stream_unknown_names.rs"]
mod metric_stream_unknown_names;
#[path = "all/metric_streams.rs"]
mod metric_streams;
#[path = "all/metrics_and_alarms.rs"]
mod metrics_and_alarms;
#[path = "all/missing_series_alarms.rs"]
mod missing_series_alarms;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/otel_enrichment.rs"]
mod otel_enrichment;
#[path = "all/put_metric_data_validation.rs"]
mod put_metric_data_validation;
#[path = "all/query_form_metrics.rs"]
mod query_form_metrics;
#[path = "all/rds_alarm_action_delivery.rs"]
mod rds_alarm_action_delivery;
#[path = "all/replay_probe.rs"]
mod replay_probe;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/sns_action_queue_policy.rs"]
mod sns_action_queue_policy;
#[path = "all/sns_action_topic_policy.rs"]
mod sns_action_topic_policy;
#[path = "all/standing_alarm_suppression.rs"]
mod standing_alarm_suppression;
#[path = "all/start_metric_streams.rs"]
mod start_metric_streams;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/statistic_name_refusals.rs"]
mod statistic_name_refusals;
#[path = "all/stop_metric_streams.rs"]
mod stop_metric_streams;
