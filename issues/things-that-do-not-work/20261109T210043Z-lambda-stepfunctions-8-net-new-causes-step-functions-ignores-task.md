Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.lambda-task-credentials-rolearn-overrides-the-execution-role lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role lambda__stepfunctions.task-credentials-external-id-default-and-custom-match-succeed lambda__stepfunctions.task-credentials-external-id-mismatch-and-too-short-rejected lambda__stepfunctions.task-credentials-role-trust-excluding-execution-role-fails-task lambda__stepfunctions.task-credentials-role-without-invoke-permission-denied lambda__stepfunctions.inputpath-null-empty-effective-input-pass-lambda-activity lambda__stepfunctions.outputpath-null-empty-output-pass-lambda-activity lambda__stepfunctions.task-token-heartbeat-timeout-catcher-order-decides-winner lambda__stepfunctions.task-token-timeoutseconds-not-caught-by-heartbeat-timeout-catcher lambda__stepfunctions.lambda-waitfortasktoken-heartbeat-timeout-history-event lambda__stepfunctions.oversized-lambda-error-cause-truncated-name-still-caught lambda__stepfunctions.parameters-injected-payload-over-the-state-quota-rejected-before-the-task-runs lambda__stepfunctions.resultselector-path-missing-field-states-runtime lambda__stepfunctions.wait-secondspath-timestamppath-on-null-lambda-payload-fails-at-runtime lambda__stepfunctions.lambda-invoke-error-payload-missing-error-type-fails-lambda-unknown
Issue-Key: lambda__stepfunctions.lambda-task-credentials-rolearn-overrides-the-execution-role|run#11|AssertionError|assert desc_with["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role|run#12|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.task-credentials-external-id-default-and-custom-match-succeed|run#18|AssertionError|assert default_result["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.task-credentials-external-id-mismatch-and-too-short-rejected|run#19|AssertionError|assert control["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.task-credentials-role-trust-excluding-execution-role-fails-task|run#13|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.task-credentials-role-without-invoke-permission-denied|run#14|AssertionError|assert control["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.inputpath-null-empty-effective-input-pass-lambda-activity|run#11|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.outputpath-null-empty-output-pass-lambda-activity|run#12|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.task-token-heartbeat-timeout-catcher-order-decides-winner|_run_and_get_output#6|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.task-token-timeoutseconds-not-caught-by-heartbeat-timeout-catcher|run#7|AssertionError|assert desc_ok["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.lambda-waitfortasktoken-heartbeat-timeout-history-event|run#26|AssertionError|assert detail.get("resourceType") == "lambda", f"unexpected timeout detail: {detail!r}"
Issue-Key: lambda__stepfunctions.oversized-lambda-error-cause-truncated-name-still-caught|run#15|AssertionError|assert over["status"] == "SUCCEEDED", (
Issue-Key: lambda__stepfunctions.parameters-injected-payload-over-the-state-quota-rejected-before-the-task-runs|run#18|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: lambda__stepfunctions.resultselector-path-missing-field-states-runtime|run#16|AssertionError|assert bad.get("cause") == expected_cause, (
Issue-Key: lambda__stepfunctions.wait-secondspath-timestamppath-on-null-lambda-payload-fails-at-runtime|_assert_wait_runtime_failure#15|AssertionError|assert execution.get("cause") == expected_cause, (
Issue-Key: lambda__stepfunctions.lambda-invoke-error-payload-missing-error-type-fails-lambda-unknown|run#9|AssertionError|assert ok["status"] == "SUCCEEDED", (
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# 8 net-new causes: Step Functions ignores Task Credentials, never runs activities inside Map/Parallel, and mis-words its runtime causes

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Eight distinct root causes in the Step Functions interpreter and its error/cause text. They are separate work items; all eight must be closed.

Cause 1 of 8 — a Task state's `Credentials` member is not interpreted, so the task never runs.
AWS: a Task may carry `Credentials: {"RoleArn": "<arn>"}` or `{"RoleArn.$": "<path>"}`; Step Functions assumes that role (passing the state machine ARN as the default ExternalId, or a custom one when configured) and makes the integration call with the assumed role's credentials. With the role assumable and permitted, the task succeeds; a role whose trust policy excludes the execution role, or whose ExternalId condition does not match, fails the task with States.TaskFailed/AccessDenied rather than States.Runtime.
Emulator: every such execution ends FAILED with error `States.Runtime` and cause `state '<state name>' carries the member 'Credentials', which this emulator does not interpret, so nothing was run for it` — the member is simply unimplemented, so not even the positive control runs.
Why: cross-account and per-task role assumption is unusable, and because the interpreter reports States.Runtime the client cannot distinguish "unsupported" from a genuine authorization failure.
Fix: the Task-state interpreter's Credentials handling (role assumption with ExternalId, then the integration call under those credentials).
Traces: lambda__stepfunctions.lambda-task-credentials-rolearn-overrides-the-execution-role, lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role, lambda__stepfunctions.task-credentials-external-id-default-and-custom-match-succeed, lambda__stepfunctions.task-credentials-external-id-mismatch-and-too-short-rejected, lambda__stepfunctions.task-credentials-role-trust-excluding-execution-role-fails-task, lambda__stepfunctions.task-credentials-role-without-invoke-permission-denied.

Cause 2 of 8 — an Activity task inside a Map iteration or a Parallel branch is never scheduled.
AWS: an Activity Task inside a Map iteration or a Parallel branch is scheduled normally: GetActivityTask returns it to a worker, SendTaskSuccess completes it, and the iteration/branch carries its result onward.
Emulator: the execution ends FAILED with error `States.Runtime` and cause `state '<state name>' schedules an activity task inside a 'Map' iteration or a 'Parallel' branch, which this emulator does not wait at, so nothing was run for it`.
Why: fan-out over activity workers — a standard pattern — cannot run at all, and the state machine's InputPath/OutputPath semantics around those branches cannot be observed.
Fix: the Map/Parallel interpreters' activity scheduling and waiting.
Traces: lambda__stepfunctions.inputpath-null-empty-effective-input-pass-lambda-activity, lambda__stepfunctions.outputpath-null-empty-output-pass-lambda-activity.

Cause 3 of 8 — a States.Timeout raised by HeartbeatSeconds or TimeoutSeconds is not caught by a Catch naming States.Timeout.
AWS: a Catch whose ErrorEquals includes States.Timeout catches a task-token state's heartbeat timeout and its TimeoutSeconds breach, transitioning to the Next state so the execution can finish successfully; catcher ordering decides which catcher wins when several match.
Emulator: the execution ends FAILED with error States.Timeout even though a matching Catch is present — the timeout is raised outside the catch machinery.
Why: every timeout-recovery branch in a state machine is dead; a workflow AWS would recover from fails the whole execution.
Fix: the task-token timeout path — raise the timeout as a catchable error so the state's Catch/Retry rules are evaluated.
Traces: lambda__stepfunctions.task-token-heartbeat-timeout-catcher-order-decides-winner, lambda__stepfunctions.task-token-timeoutseconds-not-caught-by-heartbeat-timeout-catcher.

Cause 4 of 8 — the heartbeat-timeout history event carries no timeout detail.
AWS: the history event for a heartbeat timeout carries the timeout details (its error and cause) so a client can see which state timed out and why.
Emulator: the timeout detail is an empty object.
Why: a client reading GetExecutionHistory to attribute a timeout has nothing to read, so an operator cannot tell a heartbeat timeout from any other failure.
Fix: the execution-history writer for the heartbeat-timeout event.
Traces: lambda__stepfunctions.lambda-waitfortasktoken-heartbeat-timeout-history-event.

Cause 5 of 8 — an oversized Lambda error cause is not caught by name.
AWS: when a Lambda task fails with an error whose cause is far larger than the 256 KiB state-payload quota, the error name is still matched by a Catch, the cause is truncated, and the execution SUCCEEDS through the Catch transition.
Emulator: the execution ends FAILED — the oversized cause prevents the Catch from matching.
Why: a handler that fails with a large payload (a stack trace or a response body) defeats the state machine's error handling entirely, instead of being caught and truncated.
Fix: the Lambda-integration error path — truncate the cause to the quota before error matching, and keep the error name intact.
Traces: lambda__stepfunctions.oversized-lambda-error-cause-truncated-name-still-caught.

Cause 6 of 8 — the over-quota payload refusal uses non-AWS wording.
AWS: a Parameters-injected payload larger than the state quota fails the state before the task runs, with cause `The state/task '<state name>' provided parameters with a size exceeding the maximum number of bytes service limit.` — fixed text with the state's name from the definition substituted.
Emulator: answers `a payload template produced a document of at least <n> bytes, more than the <quota> a state's output may carry` — its own sentence, naming neither the state nor AWS's limit phrasing, and describing it as an output rather than parameters.
Why: a client that matches the cause (or shows it to an operator) cannot identify the offending state, and error-taxonomy assertions on AWS's sentence fail.
Fix: the payload-template size check's cause serializer.
Traces: lambda__stepfunctions.parameters-injected-payload-over-the-state-quota-rejected-before-the-task-runs.

Cause 7 of 8 — States.Runtime causes omit AWS's "An error occurred while executing the state ..." prefix.
AWS: a path-resolution failure fails the execution with error States.Runtime and a cause that opens with `An error occurred while executing the state '<state name>' (entered at the event id #<id>). ` and continues with the specific reason — e.g. `The JSONPath '<path>' specified for the field '<field>' could not be found in the input '<input JSON>'` for a ResultSelector/Parameters path, and `The <field> parameter does not reference an input value: <path>` for a Wait state's SecondsPath/TimestampPath. The fixed parts are that prefix and the reason templates; the variable parts are the state name, the history event id, the path, the field name and the input document (serialised as AWS serialises it, with a space after each `:`).
Emulator: emits only the reason, with no prefix and no state/event id, reproduces the input without AWS's spacing, and for the Wait state substitutes a different sentence (`Invalid path '<path>' : No results for path: <path>. The '<field>' field selected no value.`).
Why: the prefix is what tells a client which state and which history event failed; without it a failure cannot be located in a large state machine, and any assertion on AWS's cause fails.
Fix: the States.Runtime cause serializer in the interpreter — prepend AWS's state/event prefix and use AWS's reason templates and input serialisation.
Traces: lambda__stepfunctions.resultselector-path-missing-field-states-runtime, lambda__stepfunctions.wait-secondspath-timestamppath-on-null-lambda-payload-fails-at-runtime.

Cause 8 of 8 — a Lambda task against a provided.al2023 function fails the execution with Lambda.InvalidRuntimeException.
AWS: a Task invoking a function on the provided.al2023 (OS-only) runtime runs the function's bootstrap and returns its response; the positive control succeeds, and only then is the error-payload shaping under test exercised.
Emulator: the task fails with error `Lambda.InvalidRuntimeException` and cause `The runtime provided.al2023 is not supported by this emulator, which runs the Python and node runtimes. (Service: AWSLambda; Status Code: 502; Error Code: InvalidRuntimeException; Request ID: <id>; Proxy: null)` — Lambda refuses the invoke, so no state-machine behavior is reached.
Why: any state machine whose task targets a custom-runtime function cannot run, and the client is handed a 502-class error for a perfectly valid runtime.
Fix: Lambda's invoke path must execute provided.al2023 (OS-only runtime) functions; the Step Functions Lambda integration then surfaces the function's real result.
Traces: lambda__stepfunctions.lambda-invoke-error-payload-missing-error-type-fails-lambda-unknown.

## Evidence
Cause 1: AssertionError: ... got 'FAILED' error='States.Runtime' cause="state '<state>' carries the member 'Credentials', which this emulator does not interpret, so nothing was run for it" (6 checks).
Cause 2: AssertionError: the fan-out execution must SUCCEED, got 'FAILED' error='States.Runtime' cause="state 'ActivityBranch' schedules an activity task inside a 'Map' iteration or a 'Parallel' branch, which this emulator does not wait at, so nothing was run for it" (2 checks).
Cause 3: AssertionError: a matched Catch must let the execution finish successfully, got status='FAILED': 'States.Timeout' / "state 'WaitForToken' sent no heartbeat for its task token within the 5 seconds its 'HeartbeatSeconds' allows"; and ... "ran its task token for longer than the 5 seconds its 'TimeoutSeconds' allows".
Cause 4: AssertionError: unexpected timeout detail: {}.
Cause 5: AssertionError: a 307200-byte error, well past the 256 KiB state payload quota, must still be caught by name and SUCCEED via the Catch transition, got 'FAILED'.
Cause 6: AssertionError: expected cause "The state/task 'Invoke' provided parameters with a size exceeding the maximum number of bytes service limit.", got "a payload template produced a document of at least 263170 bytes, more than the 262144 a state's output may carry".
Cause 7: AssertionError: expected cause 'An error occurred while executing the state \'BadInvoke\' (entered at the event id #4). The JSONPath \'$.count\' specified for the field \'count.$\' could not be found in the input \'{"product": "bananas"}\'', got 'The JSONPath \'$.count\' ... in the input \'{"product":"bananas"}\''; and expected "An error occurred while executing the state 'Wait' (entered at the event id #7). The SecondsPath parameter does not reference an input value: $.val.seconds", got "... Invalid path '$.val.seconds' : No results for path: $.val.seconds. The 'SecondsPath' field selected no value."
Cause 8: AssertionError: positive control (a normal response) must succeed, got 'FAILED' error='Lambda.InvalidRuntimeException' cause='The runtime provided.al2023 is not supported by this emulator ... (Service: AWSLambda; Status Code: 502; Error Code: InvalidRuntimeException; Request ID: <id>; Proxy: null)'.
(state names, event ids, byte counts, seconds and request ids are per-run/definition values; the templates and classifications are the requirements.)

## Steps to Reproduce
Cause 1: StartExecution on a definition whose Task carries Credentials.RoleArn (static and .$ forms), with the task role assumable by the execution role.
Cause 2: StartExecution on a definition with a Parallel branch (or Map iteration) containing an Activity Task; poll GetActivityTask.
Cause 3: StartExecution on a task-token state with HeartbeatSeconds (and separately TimeoutSeconds) and a Catch on States.Timeout; let it time out.
Cause 4: Same as cause 3, then read the heartbeat-timeout event's details in GetExecutionHistory.
Cause 5: Have the Lambda task fail with a >256 KiB cause and Catch its error name.
Cause 6: Give a Task a Parameters payload template that renders over 262144 bytes.
Cause 7: Use a ResultSelector with a path missing from the input, and a Wait state with SecondsPath pointing at a null payload; compare the causes.
Cause 8: StartExecution on a Task invoking a provided.al2023 function.
