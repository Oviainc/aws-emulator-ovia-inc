Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.activity-resource-wait-for-task-token-suffix-rejected stepfunctions.activity-task-timeout-starts-at-the-claim-not-the-schedule stepfunctions.retry-intervals-grow-by-backoffrate-and-clamp-at-maxdelayseconds stepfunctions.get-activity-task-blocks-until-execution-schedules-task stepfunctions.test-state-executes-single-state-without-execution
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.activity-resource-wait-for-task-token-suffix-rejected|run#10|AssertionError|assert "$$.Task.Token" in cause and "could not be found" in cause, (
Issue-Key: stepfunctions.activity-task-timeout-starts-at-the-claim-not-the-schedule|run#9|AssertionError|assert current["status"] == "RUNNING", (
Issue-Key: stepfunctions.retry-intervals-grow-by-backoffrate-and-clamp-at-maxdelayseconds|run#10|AssertionError|assert desc.get("error") == "States.Timeout", (
Issue-Key: stepfunctions.get-activity-task-blocks-until-execution-schedules-task|run#5|AssertionError|assert not future.done(), "GetActivityTask returned before any work was scheduled"
Issue-Key: stepfunctions.test-state-executes-single-state-without-execution|run#15|AssertionError|assert message == "Missing required field: 'Type'", f"unexpected InvalidDefinition message: {message!r}"
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Step Functions activities and TestState diverge: Task states for activity resources never execute (and an unclaimed task is wrongly timed out), GetActivityTask does not long-poll, and TestState validates a non-single-state definition as a wrapped state machine (3 causes)

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three causes in the Step Functions data plane, all surviving checks not owned by an existing issue.

Cause 1 of 3: Task states that reference an activity ARN are never executed, and the execution is driven to a terminal FAILED instead of waiting for a worker.
AWS: recorded behavior only for the run-time shape; the ASL contract is that a Task state naming an activity schedules the task and the execution stays RUNNING until a worker claims and completes it. AWS's documented rule for Task.TimeoutSeconds against an activity — "For Activities, the count begins when GetActivityTask receives a token and ActivityStarted is logged in the execution event history" — means an UNCLAIMED activity task has no clock, so the execution must remain RUNNING indefinitely. Likewise, a Resource that names an activity with a .waitForTaskToken suffix but no $$.Task.Token in its Parameters must fail with a cause naming the missing $$.Task.Token context field ('could not be found'), and a Task whose own TimeoutSeconds elapses after its Retry attempts are exhausted must leave error = 'States.Timeout'.
Emulator: all three collapse to the same thing — the execution goes FAILED with error='States.Runtime' and cause="state '<name>' is of type 'Task', which this emulator does not execute, so nothing was run for it". In activity-task-timeout-starts-at-the-claim-not-the-schedule the execution is already FAILED ~5 s in, with no worker having polled, where AWS leaves it RUNNING.
Why: no activity-based workflow can run; every client sees an error name ASL never produces for these conditions, so Catch/Retry keyed on States.Timeout or on the task-token diagnostic never matches, and a wrongly-terminal execution makes a long-lived human-in-the-loop step look broken.
Fix: the Step Functions interpreter's Task-state dispatch for activity resources, including its timeout clock (started at ActivityStarted, not at schedule time) and its Parameters/$$.Task.Token validation.
Traces: sfn.activity-resource-wait-for-task-token-suffix-rejected, sfn.activity-task-timeout-starts-at-the-claim-not-the-schedule, sfn.retry-intervals-grow-by-backoffrate-and-clamp-at-maxdelayseconds

Cause 2 of 3: GetActivityTask does not long-poll — it returns immediately when no task is scheduled.
AWS: Smithy model documentation for GetActivityTask: "This initiates a long poll, where the service holds the HTTP connection open and responds as soon as a task becomes available... The maximum time the service holds on to the request before responding is 60 seconds. If no task is available within 60 seconds, the poll returns a taskToken with a null string." Modeled errors: ActivityDoesNotExist, ActivityWorkerLimitExceeded, InvalidArn, Kms* (all 400).
Emulator: the call completes immediately — the check's worker future is already done before any execution scheduled work (AssertionError: GetActivityTask returned before any work was scheduled).
Why: a worker written to AWS's contract (socket timeout >= 65 s, loop on an empty taskToken) degenerates into a hot spin, and a task scheduled a second after the poll is never picked up by that poll.
Fix: the GetActivityTask handler — hold the request up to 60 s against the activity's pending-task queue and return an empty taskToken on expiry.
Traces: sfn.get-activity-task-blocks-until-execution-schedules-task
Note: 20261024T043150Z-sfn-createactivity-stub.md and the auto-filed GetActivityTask stub issue cover the operation being an unimplemented scaffold; the long-poll contract above is the behavior the implementation must have and is not stated there.

Cause 3 of 3: TestState rejects a definition that is not a single state with a state-machine-shaped diagnostic naming a synthetic state.
AWS: Smithy model — TestState.definition is the definition of ONE state (the operation also takes stateName, inspectionLevel, etc.) and models InvalidDefinition (400). Passing a whole state-machine document ({"StartAt": ..., "States": {...}}) is therefore a state object with no "Type", and AWS refuses it InvalidDefinition with the missing-required-member rule: it names the absent member of the supplied object — recorded message "Missing required field: 'Type'".
Emulator: same code (InvalidDefinition) but the request is validated as if it had been wrapped into a state machine of the emulator's own making: message "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: state 'TheStateUnderTest' must carry a string 'Type' member (at /States/TheStateUnderTest/Type)'". 'TheStateUnderTest' and the /States/... pointer are not in the request at all — the emulator's internal wrapper leaks into the response. The happy path (Pass state with Result/ResultPath, no executionArn, no persisted state machine) does pass.
Why: a client cannot locate the problem — the diagnostic points at a state name and JSON pointer that do not exist in anything it sent, and tooling that parses the pointer to highlight the offending member is misdirected.
Fix: the TestState handler's validation — validate the supplied definition as a single state and report the missing required member of that object, rather than running whole-state-machine schema validation over a synthetic wrapper.
Traces: sfn.test-state-executes-single-state-without-execution

## Evidence
Cause 1:
- sfn.activity-resource-wait-for-task-token-suffix-rejected — AssertionError: expected the cause to name the missing $$.Task.Token context field, got "state 'PauseAndWait' is of type 'Task', which this emulator does not execute, so nothing was run for it"
- sfn.activity-task-timeout-starts-at-the-claim-not-the-schedule — AssertionError: the execution went 'FAILED' after ~5s with NO worker ever polling. An unclaimed activity task must not be timed out: 'States.Runtime' / "state 'HandOff' is of type 'Task', ... nothing was run for it"
- sfn.retry-intervals-grow-by-backoffrate-and-clamp-at-maxdelayseconds — AssertionError: the surviving error after retry exhaustion must be the task's own States.Timeout; got error='States.Runtime' cause="state 'Attempt' is of type 'Task', ... nothing was run for it"

Cause 2:
- sfn.get-activity-task-blocks-until-execution-schedules-task — AssertionError: GetActivityTask returned before any work was scheduled (the check submits GetActivityTask on a worker thread and asserts `not future.done()` before any execution exists).

Cause 3:
- sfn.test-state-executes-single-state-without-execution — code InvalidDefinition matches; message diverges: expected "Missing required field: 'Type'", got "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: state 'TheStateUnderTest' must carry a string 'Type' member (at /States/TheStateUnderTest/Type)'" for TestState(definition=json of {"StartAt":"A","States":{"A":{"Type":"Pass","Next":"B"},"B":{"Type":"Pass","End":true}}}, input="{}").

## Steps to Reproduce
Cause 1: CreateActivity; CreateStateMachine with a Task state whose Resource is that activity ARN (variant a: suffix .waitForTaskToken with no $$.Task.Token in Parameters; variant b: TimeoutSeconds=10 and no worker polling; variant c: Retry with BackoffRate/MaxDelaySeconds plus TimeoutSeconds=1 and a worker that claims but never completes). StartExecution and poll DescribeExecution. Expected: (a) FAILED whose cause names the missing $$.Task.Token context field, (b) still RUNNING well past TimeoutSeconds, (c) terminal error States.Timeout. Actual in all three: FAILED with error States.Runtime and the 'does not execute Task states' cause.
Cause 2: CreateActivity, then call GetActivityTask on a background thread with no execution in existence. Expected: the call blocks up to 60 s and then returns an empty taskToken. Actual: it returns immediately.
Cause 3: call TestState with definition set to a full state-machine document ({"StartAt":"A","States":{...}}) and input "{}". Expected: InvalidDefinition naming the missing required member of the supplied state object ("Missing required field: 'Type'"). Actual: InvalidDefinition whose message reports a state-machine schema violation at /States/TheStateUnderTest/Type, a state name and path absent from the request.
