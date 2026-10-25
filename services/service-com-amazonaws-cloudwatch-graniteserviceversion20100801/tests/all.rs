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
#[path = "all/alarm_action_dispatch.rs"]
mod alarm_action_dispatch;
#[path = "all/alarm_action_failures.rs"]
mod alarm_action_failures;
#[path = "all/alarm_actions_switch.rs"]
mod alarm_actions_switch;
#[path = "all/alarm_configuration.rs"]
mod alarm_configuration;
#[path = "all/alarm_evaluation.rs"]
mod alarm_evaluation;
#[path = "all/alarm_history.rs"]
mod alarm_history;
#[path = "all/alarm_history_data.rs"]
mod alarm_history_data;
#[path = "all/alarm_reason_datapoints.rs"]
mod alarm_reason_datapoints;
#[path = "all/composite_alarms.rs"]
mod composite_alarms;
#[path = "all/dashboards.rs"]
mod dashboards;
#[path = "all/datasets.rs"]
mod datasets;
#[path = "all/ec2_alarm_actions.rs"]
mod ec2_alarm_actions;
#[path = "all/forced_state_actions.rs"]
mod forced_state_actions;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/insight_rules.rs"]
mod insight_rules;
#[path = "all/log_alarms.rs"]
mod log_alarms;
#[path = "all/managed_insight_rules.rs"]
mod managed_insight_rules;
#[path = "all/metric_data.rs"]
mod metric_data;
#[path = "all/metric_math.rs"]
mod metric_math;
#[path = "all/metric_streams.rs"]
mod metric_streams;
#[path = "all/metrics_and_alarms.rs"]
mod metrics_and_alarms;
#[path = "all/query_form_metrics.rs"]
mod query_form_metrics;
#[path = "all/replay_probe.rs"]
mod replay_probe;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/sns_action_queue_policy.rs"]
mod sns_action_queue_policy;
#[path = "all/sns_action_topic_policy.rs"]
mod sns_action_topic_policy;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
