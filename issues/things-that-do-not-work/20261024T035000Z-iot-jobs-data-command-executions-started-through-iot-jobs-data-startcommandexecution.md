Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected|_poll#13|Timeout|time.sleep(interval_s)
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Command executions started through iot-jobs-data StartCommandExecution are not readable back, so both command-execution lifecycle checks stall until timeout

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCommandExecution

## Description
Cause 1 of 1 — an execution minted by iot-jobs-data `StartCommandExecution` cannot be read back through the IoT control plane, so no check can observe a command execution's state, and both failing checks hang in their read loop until the harness kills them at 60s.

Failing checks for this cause: iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive, iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected.

AWS: `StartCommandExecution` (Smithy, iotjobsdataplane: input targetArn (TargetArn, len 0..2048, required), commandArn (CommandArn, required), clientToken (ClientRequestTokenV2, @idempotencyToken), executionTimeoutSeconds (range 1..); output executionId (CommandExecutionId, pattern ^[a-zA-Z0-9_-]+$)) creates a command-execution record that is IMMEDIATELY readable through the IoT control plane: `GetCommandExecution(executionId, targetArn)` returns HTTP 200 with that record and `ListCommandExecutions(commandArn, startedTimeFilter)` lists it. Smithy (iot): `CommandExecutionStatus` = CREATED | IN_PROGRESS | SUCCEEDED | FAILED | REJECTED | TIMED_OUT — a freshly started execution rests at `CREATED`, and one no device picks up moves to `TIMED_OUT` once `executionTimeoutSeconds` elapses. Recorded behavior (both checks, measured us-west-2 2026-09-11) confirms: start, then poll the control-plane read until the record appears with status CREATED/TIMED_OUT.

Emulator: the data-plane start succeeds (both checks get past it — the idempotency leg in the first check asserts the same clientToken returns the same executionId BEFORE the stall, and that assertion did not fire), but the control-plane reads that make the execution observable are unimplemented scaffold stubs answering HTTP 501 `NotImplemented`: `services/service-com-amazonaws-iot-awsiotservice/src/ops/get_command_execution.rs` and the corresponding `list_command_executions.rs`. Both checks' read loops swallow the client error and retry, so neither can ever converge and each is reported as `Check timed out after 60.0s` rather than as a response mismatch. Nothing downstream of the first read (the ghost/absent-target TIMED_OUT leg, the deprecate/undeprecate rejection legs, the "refusals created no execution" leg) is exercised at all.

Why: a client that starts a command execution and then polls for its progress — the normal device-command workflow — gets a 501 from every read and can never learn an execution's status, its commandArn/targetArn, or whether an execution exists. The data plane reports success for state that is unreachable, which is worse than a clean refusal: the two planes disagree about what exists.

Fix: implement the IoT control-plane command-execution readers over the SAME shared store the data plane writes (`service_common::iot_job_executions` / `crate::state` records written by `ops/start_command_execution.rs`), so an execution minted on the data plane is visible on the control plane: `services/service-com-amazonaws-iot-awsiotservice/src/ops/get_command_execution.rs` and `.../ops/list_command_executions.rs`. Two open issues already name exactly these two stubs for service `iot` — "Implement iot GetCommandExecution: unimplemented scaffold stub" (20261023T110949Z-iot-getcommandexecution-stub.md) and "Implement iot ListCommandExecutions: unimplemented scaffold stub" (20261023T110949Z-iot-listcommandexecutions-stub.md) — but neither carries these two iot-jobs-data check ids as replay keys, so this issue is what gates the cross-plane behavior those checks grade. Hypothesis (visible in `ops/start_command_execution.rs` but NOT observable in this replay, because the reader is a 501): the handler persists the new record via `JobExecutionRecord::queued(..)`, i.e. status QUEUED; the control-plane reader must project a newly started command execution as `CREATED` and must materialize the `executionTimeoutSeconds` timer into `TIMED_OUT`, otherwise these loops will still never converge after the stubs are implemented.

Traces: iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive (stalls on the first `ListCommandExecutions(commandArn=..., startedTimeFilter=...)` poll asserting exactly one execution), iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected (stalls on the first `GetCommandExecution(executionId, targetArn)` poll waiting for status CREATED).

## Evidence
Suite report (simparity-behavioral-ga-prod-v1): iot-jobs-data 3/5 (60%), 2 failed.
  [mismatch] iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive — "Error: Check timed out after 60.0s"
  [mismatch] iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected — "Error: Check timed out after 60.0s"

Expected (AWS): after StartCommandExecution returns an executionId, GetCommandExecution(executionId, targetArn) answers 200 with status CREATED (then TIMED_OUT when the executionTimeoutSeconds timer expires with no device pickup), and ListCommandExecutions(commandArn, startedTimeFilter={after: ...}) lists exactly the executions started for that command.

Actual (emulator): both control-plane readers are unmodified scaffolds. `services/service-com-amazonaws-iot-awsiotservice/src/ops/get_command_execution.rs` carries the scaffold marker and answers HTTP 501 `NotImplemented` ("operation GetCommandExecution is scaffolded but not implemented in this emulator"), as does ListCommandExecutions. Both checks wrap the read in a retry loop that catches ClientError, so the 501 is retried until the harness's 60s cap, producing a timeout verdict instead of a response diff.

Corroboration that the data-plane half runs: `services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_command_execution.rs` is fully implemented (validates targetArn/commandArn, honors clientToken idempotency, mints and persists a record), and the first check's pre-poll assertions (second executionId == first) did not fire — the failure is purely on the read-back path.

## Steps to Reproduce
1. Boot the emulator and resolve the iot-jobs-data endpoint via iot DescribeEndpoint(endpointType="iot:Data-ATS").
2. iot CreateThing(thingName=T); iot DescribeThing(T) -> thingArn.
3. iot CreateCommand(commandId=C, namespace="AWS-IoT", payload={content: b'{"reboot": true}', contentType: "application/json"}) -> commandArn.
4. Against the iot-jobs-data endpoint: StartCommandExecution(targetArn=thingArn, commandArn=commandArn) -> note the returned executionId (HTTP 200).
5. iot GetCommandExecution(executionId=<that id>, targetArn=thingArn) -> observe HTTP 501 NotImplemented instead of 200 with status CREATED.
6. iot ListCommandExecutions(commandArn=commandArn, startedTimeFilter={"after": "<YYYY-MM-DDThh:mm>"}) -> observe HTTP 501 NotImplemented instead of a page containing that executionId.
7. Replay iot-jobs-data.start-command-execution-unknown-and-deprecated-command-rejected and iot-jobs-data.start-command-execution-client-token-idempotent-and-absent-target-over-permissive: both hang in the read loop and are killed at 60s.
