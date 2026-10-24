Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# ExecuteScheduledReport triggers an execution that delivers no notification to the default EventBridge event bus

_Filed: 2026-10-24T09:36:42Z_
_Operation: ExecuteScheduledReport | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An execution of a scheduled report is, per the model, a delivery of a notification to the account's default Amazon EventBridge event bus: `HealthStatus.statusCode` documents UNHEALTHY as "the scheduled report is unable to deliver the notification to the default Amazon Web Services EventBridge EventBus in your account", i.e. a healthy execution DOES deliver that notification, and ExecuteScheduledReport "Triggers an immediate execution of a scheduled report, outside of its regular schedule".

The handler treats the execution as purely internal bookkeeping: on the triggered path it writes `stored.health_status` and `stored.last_execution_at` and records an audit line, and nothing is ever delivered anywhere. No event is put on the emulated EventBridge default bus (the repo mounts an EventBridge emulator with PutEvents and event-bus state, and cross-service reads/writes between service crates are an established pattern here, e.g. bedrock-agentcore→Lambda and Amplify→S3). Expected: a non-dryRun ExecuteScheduledReport on an ENABLED, HEALTHY report emits one event onto the caller's default event bus (source identifying BCM Dashboards, detail naming the scheduled-report ARN, the dashboard ARN and the execution time), so that a client wiring an EventBridge rule to a scheduled report observes the delivery; actual: nothing is emitted, so the entire scheduled-report → EventBridge → target workflow is unobservable, and the UNHEALTHY/healthy distinction the model grounds in delivery success is simulated by resource-existence checks alone.

## Evidence

Spec: smithy_query(query="shape", shape="HealthStatus") — statusCode doc: "UNHEALTHY indicates the scheduled report is unable to deliver the notification to the default Amazon EventBridge EventBus in your account"; smithy_query(query="doc", shape="ExecuteScheduledReport") — "Triggers an immediate execution of a scheduled report, outside of its regular schedule"; output member `executionTriggered` is "Indicates whether the execution was successfully triggered". Handler: src/ops/execute_scheduled_report.rs lines 286-333 — after computing `health_status` it only mutates `stored.health_status`/`stored.last_execution_at`, inserts the idempotency record, and calls `crate::state::record_state_change`; there is no cross-service call at all (contrast src/ops/create_gateway_target.rs in the bedrockagentcorecontrol crate, which drops its own guard and performs a cross-service read). State: src/state.rs holds only dashboards, scheduled reports and token maps — no outbound notification record. Tests: tests/all/scheduled_report_execution.rs asserts only the returned healthStatus/executionTriggered and the stored lastExecutionAt; nothing asserts any delivery, so the gap is uncovered.

## Steps to Reproduce

1. CreateDashboard, then CreateScheduledReport against that dashboard with scheduleConfig {scheduleExpression: "cron(0 8 1 * ? *)"} (state defaults to ENABLED); note the scheduled-report ARN.
2. Against the emulated EventBridge service, confirm the account's `default` event bus exists (DescribeEventBus with no Name).
3. Call ExecuteScheduledReport with that ARN and no dryRun — observe HTTP 200 with executionTriggered=true and healthStatus HEALTHY.
4. Inspect the default event bus for an event produced by the execution (the delivery the HealthStatus doc describes) — expected one notification event naming the scheduled report and its dashboard; observed no event at all: the emulator records only an internal health refresh and lastExecutionAt.
