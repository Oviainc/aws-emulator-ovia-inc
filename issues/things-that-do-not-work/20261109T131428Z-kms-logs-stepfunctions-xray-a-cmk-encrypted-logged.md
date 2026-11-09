Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__logs__stepfunctions__xray.disabled-cmk-blocks-new-logged-and-traced-execution kms__logs__stepfunctions__xray.cmk-encrypted-execution-still-delivers-to-cloudwatch-logs-and-xray
Issue-Key: kms__logs__stepfunctions__xray.disabled-cmk-blocks-new-logged-and-traced-execution|_await_state_machine_segment#10|TimeoutError|raise TimeoutError(
Issue-Key: kms__logs__stepfunctions__xray.cmk-encrypted-execution-still-delivers-to-cloudwatch-logs-and-xray|_await_trace#22|Timeout|time.sleep(_POLL_S)
Issue-Commit: fb003e0a3fdd2a31392ea928ed9540deb9fe4c34

# kms__logs__stepfunctions__xray: a CMK-encrypted, logged and tracingConfiguration-enabled state machine publishes no X-Ray segment for its execution (2 causes)

> _Note: this issue was identified against the code base at commit `fb003e0a3fdd2a31392ea928ed9540deb9fe4c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
Two net-new root causes behind 2 failing checks.

Cause 1 of 2: an execution of a state machine created with tracingConfiguration.enabled=true publishes NO segment to X-Ray — the execution's trace id is retrievable as a trace with no Step Functions segment in it (segment origins read []), so nothing in X-Ray records that the execution ran. Failing check: kms__logs__stepfunctions__xray.disabled-cmk-blocks-new-logged-and-traced-execution.

AWS: with tracingConfiguration.enabled=true (Smithy sfn: TracingConfiguration.enabled, boolean, default false, "When set to true, X-Ray tracing is enabled"), Step Functions publishes the execution's trace to X-Ray under the execution's Root trace id. BatchGetTraces for that Root id (Smithy xray: BatchGetTraces POST /Traces, output Traces/UnprocessedTraceIds) returns a trace whose Segments contain at least one segment document with origin "AWS::StepFunctions::StateMachine". The Root id is minted per execution and is never itself a requirement; the requirement is that for a traced execution some segment with that origin becomes retrievable, and for an untraced one none does.

Emulator: the check's enabled-key control execution succeeds, but polling X-Ray for its Root id never yields a Step Functions segment: `TimeoutError: the enabled-key execution's X-Ray trace 1-<8 hex>-<24 hex> had no Step Functions segment when the run budget ended; segment origins read []; not graded`. The origin set is empty, i.e. no segment at all is ingested for the execution — tracingConfiguration.enabled is an observable no-op.

Why: a client enabling X-Ray tracing on a state machine gets no tracing data while the control plane keeps reporting tracing as enabled. It cannot find an execution's trace, cannot see which states contributed, and cannot distinguish a sampling decision from a missing integration; any KMS/logging/tracing conformance test that uses the trace as proof the execution ran can never pass.

Fix: the trace-publication path of the sfn service crate's execution runtime and its X-Ray integration — when tracingConfiguration.enabled is true, emit the execution's root segment (origin AWS::StepFunctions::StateMachine) under the execution's Root trace id to X-Ray's segment store so BatchGetTraces/GetTraceSummaries resolve it, and publish nothing when it is false.

Cause 2 of 2: the CMK-encrypted happy-path twin never settles at all — the execution's logged history and its trace never both become readable, so the check exhausts the suite budget (Check timed out after 180.0s) instead of reaching a verdict. The same X-Ray publication gap of Cause 1 is on its critical path (it waits for a trace carrying an AWS::StepFunctions::StateMachine segment for the Root id StartSyncExecution echoed back), and the wait that precedes it is CloudWatch Logs delivery of the CMK-encrypted execution's ExecutionStarted/ExecutionSucceeded history. Failing check: kms__logs__stepfunctions__xray.cmk-encrypted-execution-still-delivers-to-cloudwatch-logs-and-xray.

AWS: an Express state machine with encryptionConfiguration type CUSTOMER_MANAGED_KMS_KEY (key policy granting delivery.logs.amazonaws.com kms:Decrypt) run with LogLevel ALL delivers its decrypted ExecutionStarted input and its ExecutionSucceeded history to the configured log group, AND, with tracing enabled, publishes the execution's trace to X-Ray; StartSyncExecution echoes the supplied traceHeader's Root id. Recorded behavior only for the exact log-record shapes; the requirement is that encryption at rest does not suppress either sink.

Emulator: the check produces no verdict within the budget — one of the two deliveries (the X-Ray segment of Cause 1, and/or the decrypted history records in CloudWatch Logs) never arrives.

Why: a client that encrypts its state machine with a CMK cannot verify that logging and tracing still work; a hang with no error is worse than a refusal, because nothing tells the caller which sink is missing.

Fix: same publication path as Cause 1, plus the sfn crate's CloudWatch Logs delivery for CMK-encrypted executions (the decrypted ExecutionStarted input and the terminal ExecutionSucceeded record must reach the configured log group).

Traces: kms__logs__stepfunctions__xray.disabled-cmk-blocks-new-logged-and-traced-execution; kms__logs__stepfunctions__xray.cmk-encrypted-execution-still-delivers-to-cloudwatch-logs-and-xray. Related open issue 20261106T214711Z-stepfunctions-xray-a-tracingconfiguration-enabled-state-machine-publishes.md records the same publication gap under the stepfunctions__xray heading and does not carry these checks' replay keys.

## Evidence
Cause 1 — Expected (AWS): BatchGetTraces(TraceIds=[<Root id of a tracing-enabled execution>]) returns a trace whose segment documents include one with origin "AWS::StepFunctions::StateMachine".
Actual (emulator): TimeoutError: the enabled-key execution's X-Ray trace 1-6af1c5e4-6604077454fb0850ba92f214 had no Step Functions segment when the run budget ended; segment origins read []; not graded.

Cause 2 — Expected (AWS): a CUSTOMER_MANAGED_KMS_KEY-encrypted Express execution SUCCEEDS, its decrypted ExecutionStarted input and ExecutionSucceeded record appear in the configured log group, and its echoed Root trace id resolves at X-Ray to a non-empty Segments list containing an AWS::StepFunctions::StateMachine origin.
Actual (emulator): Error: Check timed out after 180.0s — neither sink's wait completes, so no verdict is produced.

## Steps to Reproduce
1. CreateKey with a policy granting the account root kms:* and delivery.logs.amazonaws.com kms:Decrypt.
2. Create an IAM role trusted by states.amazonaws.com allowing kms:Decrypt/GenerateDataKey on the key, xray:PutTraceSegments/PutTelemetryRecords/GetSamplingRules/GetSamplingTargets and the logs:* delivery actions.
3. CreateStateMachine (type EXPRESS) with encryptionConfiguration {type: CUSTOMER_MANAGED_KMS_KEY, kmsKeyId: <key arn>, kmsDataKeyReusePeriodSeconds: 60}, loggingConfiguration level ALL to a log group, and tracingConfiguration {enabled: true}.
4. StartSyncExecution supplying a trace header with Sampled=1; capture the echoed traceHeader's Root id.
5. Poll CloudWatch Logs for the execution's ExecutionStarted/ExecutionSucceeded records, and BatchGetTraces for the Root id.
6. Expected: history records present, and a trace segment with origin AWS::StepFunctions::StateMachine. Observed: segment origins [] (no segment ever ingested); the happy-path twin never settles within the budget.
