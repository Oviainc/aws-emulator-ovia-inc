Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-choice-on-missing-status-path-rejected-at-runtime events__lambda__stepfunctions.refarch-sfn-nonexistent-role-accepted-at-create-rejected-on-data-plane
Issue-Key: events__lambda__stepfunctions.refarch-sfn-choice-on-missing-status-path-rejected-at-runtime|run#18|AssertionError|assert "JobComplete" in cause, (
Issue-Key: events__lambda__stepfunctions.refarch-sfn-nonexistent-role-accepted-at-create-rejected-on-data-plane|run#19|AssertionError|assert denied["status"] == "FAILED", (
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# events__lambda__stepfunctions: a Choice state's runtime failure cause does not name the state or field, and a state machine with a nonexistent execution role still SUCCEEDS (2 causes)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes. Related open issue 20261027T000406Z-sfn-step-functions-choice-and-wait-evaluation-diverges-numericequals.md carries the same cause-wording defect for checks under the `stepfunctions` heading; neither of the checks below is owned by it or by any other open issue.

Cause 1 of 2: when a Choice rule's path selects no value, the failure cause does not identify the Choice state (nor the field) that could not be evaluated.
AWS: an unresolvable comparison path fails the execution with error `States.Runtime` and a cause that names the state being executed, the event id at which it was entered and the offending path — the fixed text is "An error occurred while executing the state '<StateName>' (entered at the event id #<id>). Invalid path '<path>' : No results for path: <path>", with the state name, event id and path taken from the definition and the execution. Smithy sfn#HistoryEventType models ExecutionFailed/EvaluationFailed as the carriers of that error/cause pair; the cause member (SensitiveCause, max 32768) is where the diagnosis lives.
Emulator: the cause is `the path '$.status' selected no value` — emulator-authored wording that names only the path and never the state, so in a definition with many Choice states a caller cannot tell which one failed. The error code itself (States.Runtime) is right; the message is not.
Why: the only machine-readable handle on the failure is the cause string. Operators and automated triage that parse it for the state name — exactly what AWS's wording supports — get nothing, and the failure is indistinguishable from any other States.Runtime.
Fix: the Step Functions ASL interpreter's Choice-state evaluation (and the cause formatter it shares with the other states) must render AWS's "An error occurred while executing the state '<name>' (entered at the event id #<id>). Invalid path ... : No results for path: ..." text.
Traces: events__lambda__stepfunctions.refarch-sfn-choice-on-missing-status-path-rejected-at-runtime.

Cause 2 of 2: a state machine created with a roleArn that does not exist is accepted at create (correctly) but its executions SUCCEED instead of failing on the data plane.
AWS: CreateStateMachine does not require the role to exist, but Step Functions assumes the roleArn through sts:AssumeRole before dispatching any Task. When the role does not exist the assume fails and the execution ends FAILED — it never reaches SUCCEEDED and no Task side effect is applied.
Emulator: DescribeExecution reports `status: SUCCEEDED` for an execution whose role cannot be assumed; no role resolution or trust evaluation happens at execution start.
Why: the deploy-time/run-time split AWS deliberately exposes (accepted at create, refused at run) is lost, so a deployment with a dangling role reference looks healthy, and the emulator applies side effects AWS would never perform.
Fix: the Step Functions execution-start path must resolve the state machine's roleArn (existence plus trust for states.amazonaws.com) and fail the execution when it cannot be assumed.
Traces: events__lambda__stepfunctions.refarch-sfn-nonexistent-role-accepted-at-create-rejected-on-data-plane.

## Evidence
Cause 1 — expected: the cause names the Choice state that could not be evaluated. Actual: `AssertionError: the cause should name the Choice state that could not be evaluated; got "the path '$.status' selected no value"`.

Cause 2 — expected: an execution whose role cannot be assumed must FAIL. Actual: `AssertionError: an execution whose role cannot be assumed must FAIL; got 'SUCCEEDED'`.

## Steps to Reproduce
Cause 1: Create a state machine whose Choice state tests `$.status` with StringEquals and declares no Default, start an execution whose input has no `status` member, and read DescribeExecution's error/cause — the cause is "the path '$.status' selected no value" and names no state.
Cause 2: CreateStateMachine with roleArn pointing at a role that was never created (accepted), then StartExecution and poll DescribeExecution — observe SUCCEEDED instead of FAILED.
