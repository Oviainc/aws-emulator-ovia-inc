Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# notificationConfiguration is stored but no EventBridge job-completion event is ever published

_Filed: 2026-10-24T02:47:52Z_
_Operation: InvokeDataAutomationAsync | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A submission may carry `notificationConfiguration.eventBridgeConfiguration.eventBridgeEnabled = true`, which in the real service is the documented, polling-free way to learn that an asynchronous data-automation job finished: Bedrock Data Automation publishes a job-status-change event onto the account's default EventBridge bus (source `aws.bedrock`), naming the invocation, so a rule on that bus delivers it to the caller's target.

The emulator accepts and persists the member (`Invocation::notification_configuration`) but nothing ever reads it: no code path publishes anything to EventBridge when a job's lifecycle reaches a terminal state. A client that submits with `eventBridgeEnabled: true`, creates an EventBridge rule and waits for the completion event waits forever — the only way to observe completion here is polling `GetDataAutomationStatus`, and with `eventBridgeEnabled: false` versus `true` the emulator behaves identically, so the member has no observable effect at all.

## Evidence

Spec: `smithy_query` shape `NotificationConfiguration` has required member `eventBridgeConfiguration` -> `EventBridgeConfiguration` with required `eventBridgeEnabled` (Boolean), bound to `InvokeDataAutomationAsync` (OPERATIONS.md line 54); the member exists for no purpose other than EventBridge delivery of job status changes. Handler: `src/ops/invoke_data_automation_async.rs` line 258 stores `invocation.notification_configuration = input.notification_configuration;` and `grep -r notification_configuration` over the crate shows the field is only written and never read — `src/state.rs` `Invocation::advance_to` (the one place a job becomes terminal) publishes nothing, and the crate has no EventBridge client or `ctx.callback()` use. Feasible, established pattern in this repo: `services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs` `publish_job_state_changes` / `publish_pending` (lines ~2909-2997) publish job state changes onto the default bus through `smithy_server_runtime::OperationContext::callback()` + `aws_sdk_eventbridge::put_events`, asserted by `tests/all/job_state_change_events.rs`; the EventBridge emulator's `PutEvents` is implemented (`services/service-com-amazonaws-cloudwatchevents-awsevents/src/ops/put_events.rs`). Test coverage: `tests/all/data_automation_invocations.rs` line 412 only sends `notificationConfiguration` as part of an accepted submission and asserts nothing about any event being delivered.

## Steps to Reproduce

1. Against an emulator with both this service and EventBridge mounted on one engine, create a rule on the default event bus matching the Bedrock Data Automation job-status-change event (source `aws.bedrock`) with a target that records deliveries.
2. POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and a body carrying {"notificationConfiguration":{"eventBridgeConfiguration":{"eventBridgeEnabled":true}}} alongside the required inputConfiguration/outputConfiguration/dataAutomationProfileArn and a project ARN.
3. Poll `GetDataAutomationStatus` until the job reports `Success`.
4. Inspect the rule's target: no event was delivered. Expected a job-status-change event on the default bus naming the invocation ARN and its terminal status. Repeating with `eventBridgeEnabled: false` produces exactly the same observable behaviour, proving the member is inert.
