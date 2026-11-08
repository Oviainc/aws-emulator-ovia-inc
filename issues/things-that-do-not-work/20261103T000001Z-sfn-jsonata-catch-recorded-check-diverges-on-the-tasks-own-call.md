Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output|run#8|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sfn: a JSONata Catch recorded check still diverges after the catcher's own defect was fixed — the task's DynamoDB call answers a refusal the recording does not carry (1 cause)

> _This is the ONE cause of
> `20261029T183117Z-sfn-interpreter-and-data-fidelity-five-causes-still-open.md`
> (now under `issues/things-that-now-work/`) that is still open. The defect that issue
> named in the catcher half of the walk was found and fixed — a JSONata catcher that
> declares no `Output` now puts out the ERROR OUTPUT rather than the state's input
> (`Interpreter::jsonata_output`) — and the catcher rule is pinned over 11 shapes in
> `tests/all/interpreter_data_fidelity.rs`, which all pass. The recorded check still
> diverges, and the evidence below says the remaining difference is in what the task's
> own CALL answered rather than in what the `Catch` then did with it. It is filed on
> its own so the bundle it came from is not recorded as working while this diverges._

## Operation
StartExecution

## Description

AWS: recorded behavior only. The recorded execution SUCCEEDS: its JSONata `Task` fails,
a `Catch` catcher takes the failure over, binds `$states.errorOutput` for its own
`Output`, and the execution carries on at the catcher's `Next`.

Emulator: the execution ends
`status='FAILED' error='DynamoDB.ResourceNotFoundException' cause='Requested resource
not found (Service: AmazonDynamoDBv2; Status Code: 400; Error Code:
ResourceNotFoundException; Request ID: <per-run>; Proxy: null)'`.

What has been ruled out — the catch path itself. `tests/all/interpreter_data_fidelity.rs`
(`a_catch_reading_the_error_output_recovers_the_execution`,
`a_catch_in_a_child_state_machine_recovers_that_execution`,
`a_catch_on_a_run_a_job_task_recovers_a_failed_child`) drives a really-served DynamoDB
refusal through 11 catcher shapes and they all recover the execution: an `Output`
reading `$states.errorOutput.Error`, one reading both `.Error` and `.Cause`, a
whole-value `Output`, an `Assign` a later state reads it through, a catcher declaring
neither member, a catcher behind a spent `Retry`, a catcher beside a state-level
`Output`, both `Resource` spellings (`arn:aws:states:::dynamodb:putItem` and
`arn:aws:states:::aws-sdk:dynamodb:putItem`/`getItem`), and the catcher on a `Map`
state, on a `Parallel` state, on the task inside a `Parallel` branch and on the task
inside a `Map` iteration — plus a `Catch` inside a state machine a `Task` STARTED and a
`Catch` on the run-a-job task whose child failed.

That leaves the error NAME the catcher has to name. `asl::errors_match` takes a failure
over for `States.ALL`, for `States.TaskFailed` (the language's other wildcard) and for
the exact service error, so a catcher naming any of those forms matches
`DynamoDB.ResourceNotFoundException`. Since the emulator's own report of the recorded
unit IS that error and the catch is reached for every shape above, the difference is
what the task's own CALL answered: either the recorded call did not fail that way (the
recorded account held a table the replayed emulator does not, or the call failed for
another reason with another error name the catcher names), or the error name this
emulator derives for that call is not the one AWS derives.

Why: an execution the real service recovers ends `FAILED`, so a definition's error
handling cannot be relied on for this integration.

Next step: obtain the recorded DEFINITION and the recorded TASK answer for
`sfn.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output` and compare, in this
order, (1) the error NAME the recording carries for the task against
`state::integration`'s derivation of it — in particular whether the `aws-sdk:` family
should take the SDK v2 service id (`DynamoDb`) where the optimized family takes the SDK
v1 one (`DynamoDB`), which `Api::sdk_id` currently uses for both — and (2) whether the
recorded call was served at all, i.e. whether the fixture's table exists in the replayed
emulator.

## Evidence
`AssertionError: the Catch did not recover the execution: 'FAILED'
error='DynamoDB.ResourceNotFoundException' cause='Requested resource not found
(Service: AmazonDynamoDBv2; Status Code: 400; Error Code: ResourceNotFoundException;
Request ID: 911d7ff7-008d-4ee9-93a7-fa2a67645124; Proxy: null)'`.

## Steps to Reproduce
Replay `sfn.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output`. The other
seven checks of the bundle this came from pass.

## Notes
Files: the catcher half of the walk is
`services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl/walk.rs`
(`Interpreter::catch_or_fail`, `Interpreter::jsonata_output`) and the error-name
derivation is `src/state/integration.rs` (`Api::sdk_id`, `sdk_exception_cause`,
`named_work`). The DynamoDB refusal itself is
`services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/state.rs`
(`item_resource_not_found`, `active_table`), which answers the same message for a table
that does not exist and for one that is still `CREATING`.
