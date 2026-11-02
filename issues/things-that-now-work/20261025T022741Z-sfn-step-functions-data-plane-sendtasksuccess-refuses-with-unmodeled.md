Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.error-invalid-output-send-task-success stepfunctions.start-execution-invalid-json-input-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.error-invalid-output-send-task-success|run#13|AssertionError|assert code == "InvalidOutput", f"expected InvalidOutput, got {code!r}"
Issue-Key: stepfunctions.start-execution-invalid-json-input-rejected|run#10|AssertionError|assert error.get("Message") == _INVALID_INPUT_MESSAGE, (
Issue-Commit: d6f5c34e1c81f18f6d6574ea7d7977035cbea7fb

# Step Functions data plane: SendTaskSuccess refuses with unmodeled NotImplemented instead of validating output, and InvalidExecutionInput's message carries a Rust parser diagnostic instead of the documented one (2 causes)

> _Note: this issue was identified against the code base at commit `d6f5c34e1c81f18f6d6574ea7d7977035cbea7fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
Two net-new root causes in the Step Functions emulator's data plane, both grounded in behavioral checks that are NOT marked ALREADY FILED or OWNED in the replay report. They share the `sfn` service and are bundled; a fixer must close BOTH before submitting.

=== Cause 1 of 2: SendTaskSuccess answers an unmodeled `NotImplemented` instead of refusing a non-JSON `output` with `InvalidOutput` (and leaving the task outstanding) ===

AWS: Smithy model (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json) binds SendTaskSuccess to inputs `taskToken` (TaskToken, required, @length 1..2048, @sensitive) and `output` (SensitiveData, required, @length 0..262144, @sensitive), and models exactly these errors, all HTTP 400: `InvalidOutput` ("The provided JSON output data is not valid"), `InvalidToken`, `KmsAccessDeniedException`, `KmsInvalidStateException`, `KmsThrottlingException`, `TaskDoesNotExist`, `TaskTimedOut`. `NotImplemented` is NOT in that set. The recorded behavior the check encodes: SendTaskSuccess with `output="not-valid-json{{{"` is refused HTTP 400 `InvalidOutput`; the refusal is non-destructive — DescribeExecution still reports the execution `RUNNING`, SendTaskHeartbeat on the SAME task token still succeeds, and a subsequent SendTaskSuccess with `output="{}"` on that same token completes the task and drives the execution to `SUCCEEDED`. Recorded example message: "Invalid Output: 'not-valid-json{{{' is not valid JSON." (message rule: built from the request — it names the member, Output, and quotes the offending value).

Emulator: the call is refused with error code `NotImplemented` (replay assertion: "expected InvalidOutput, got 'NotImplemented'"), so no output validation, no non-destructive-refusal semantics, and no callback completion exist at all.

Why: every callback-pattern client (`.waitForTaskToken`, activity workers) is broken — a worker cannot report success, so an execution that reaches a callback Task can never finish. A client that handles `InvalidOutput` to retry with corrected output instead sees an unmodeled error its SDK cannot classify, and botocore surfaces no modeled exception class for it.

Fix: the SendTaskSuccess handler and its validation in services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/send_task_success.rs — validate `output` as JSON and refuse HTTP 400 `InvalidOutput` BEFORE touching state, then complete the task through the execution/activity-task state so the token's task is resolved; a rejected call must leave the execution RUNNING and the token claimable.

Related-but-not-covering: the open issue 20261025T013638Z-sfn-sendtasksuccess-stub.md records only that SendTaskSuccess is an unimplemented scaffold stub. It does not state the InvalidOutput validation contract, the non-destructive-refusal requirement, or the token-stays-valid/completion semantics above, which is what this cause adds. The replay report did NOT mark this check as ALREADY FILED or OWNED.

Traces: sfn.error-invalid-output-send-task-success.

=== Cause 2 of 2: StartExecution's InvalidExecutionInput message embeds serde_json's diagnostic instead of the documented parser diagnostic ===

AWS: Smithy: StartExecution models `InvalidExecutionInput` (HTTP 400, "The provided JSON input data is not valid"); input member `input` is SensitiveData, @length 0..262144, and its own documentation says it "contains the JSON input data for the execution". The status (400) and the error code (`InvalidExecutionInput`) MATCH in the emulator — only the message diverges. The message rule is known because the message is built from the request: AWS wraps its JSON parser's diagnostic for the offending input in the fixed prefix "Invalid State Machine Execution Input: '<diagnostic>'", and that diagnostic NAMES the unrecognized token taken from the request and the token classes that were expected. Recorded example for input `not-json`: "Invalid State Machine Execution Input: 'Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')'".

Emulator: same prefix, but the inner diagnostic is serde_json's and names only a cursor position, not the offending token or the expected token classes — observed: "Invalid State Machine Execution Input: 'expected ident at line 1 column 2'". Everything else about the check passes: the call IS rejected, with code `InvalidExecutionInput`, and the rejected StartExecution correctly creates no execution while the positive control's execution is listed.

Why: a client or operator diagnosing a malformed payload gets no indication of WHICH token in its input was rejected or what was acceptable there; tooling and tests that assert on, log-scrape, or surface the documented diagnostic to end users read a different string for the same request. The divergence is request-derived and deterministic, so it reproduces on every malformed input, not just this one.

Fix: the StartExecution handler's input validation — services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/start_execution.rs lines 273-286 pass `error.to_string()` (the serde_json error) straight into `crate::state::resource_error_message(INVALID_STATE_MACHINE_EXECUTION_INPUT, ...)`. The diagnostic text handed to that constructor is what must follow the documented rule (name the unrecognized token from the request and the expected token classes); the same construction is shared with definition validation, so fix it where the diagnostic is produced rather than at the call site only.

Traces: sfn.start-execution-invalid-json-input-rejected.

## Evidence
Cause 1 — sfn.error-invalid-output-send-task-success [mismatch]:
  expected: SendTaskSuccess(taskToken=<valid token>, output="not-valid-json{{{") -> HTTP 400, Error.Code == "InvalidOutput" (modeled); then DescribeExecution.status == "RUNNING"; SendTaskHeartbeat(same token) succeeds; SendTaskSuccess(same token, output="{}") succeeds and DescribeExecution.status -> "SUCCEEDED".
  actual:   Error.Code == "NotImplemented" (not a modeled error for this operation).
  replay output: check_sfn_error_invalid_output_send_task_success.py line 54 -> AssertionError: expected InvalidOutput, got 'NotImplemented'
  Smithy evidence: SendTaskSuccess errors = {InvalidOutput, InvalidToken, KmsAccessDeniedException, KmsInvalidStateException, KmsThrottlingException, TaskDoesNotExist, TaskTimedOut}, all httpError 400; NotImplemented absent.

Cause 2 — sfn.start-execution-invalid-json-input-rejected [mismatch]:
  matching already: HTTP 400 and Error.Code == "InvalidExecutionInput"; the rejected call creates no execution; the valid-input positive control starts and is listed.
  expected Error.Message: "Invalid State Machine Execution Input: 'Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')'"
  actual Error.Message:   "Invalid State Machine Execution Input: 'expected ident at line 1 column 2'"
  replay output: check_sfn_start_execution_invalid_json_input_rejected.py line 116 -> AssertionError on the message.
  source: services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/start_execution.rs:273-286 injects `serde_json::Error::to_string()` as the quoted diagnostic.

## Steps to Reproduce
Cause 1 (sfn.error-invalid-output-send-task-success):
1. Create an IAM role trusted by states.amazonaws.com.
2. sfn.create_activity(name=<unique>) -> activityArn.
3. sfn.create_state_machine(definition={"StartAt":"DoTask","States":{"DoTask":{"Type":"Task","Resource":<activityArn>,"TimeoutSeconds":300,"HeartbeatSeconds":60,"End":true}}}, roleArn=<role>).
4. sfn.start_execution(stateMachineArn=<sm>, input="{}").
5. sfn.get_activity_task(activityArn=<activityArn>) until a taskToken is returned.
6. sfn.send_task_success(taskToken=<token>, output="not-valid-json{{{") -> observe Error.Code. Expected 400 InvalidOutput; emulator answers NotImplemented.
7. Then assert the refusal was non-destructive: describe_execution(...)['status'] == 'RUNNING', send_task_heartbeat(taskToken=<token>) succeeds, send_task_success(taskToken=<token>, output="{}") succeeds and the execution reaches SUCCEEDED.

Cause 2 (sfn.start-execution-invalid-json-input-rejected):
1. Create an IAM role trusted by states.amazonaws.com.
2. sfn.create_state_machine(definition={"Comment":"behavioral-pass","StartAt":"P","States":{"P":{"Type":"Pass","Result":{"ok":true},"End":true}}}, roleArn=<role>, type="STANDARD").
3. Positive control: sfn.start_execution(stateMachineArn=<sm>, name=<good>, input="{}") -> succeeds.
4. sfn.start_execution(stateMachineArn=<sm>, name=<bad>, input="not-json") -> ClientError. Code is InvalidExecutionInput (correct); compare Error.Message against the documented diagnostic quoted in the evidence.
5. sfn.list_executions(stateMachineArn=<sm>) -> <bad> absent, <good> present (both already correct).
