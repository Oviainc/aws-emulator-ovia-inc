Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.deep-sfn-3-http-task-echo-roundtrip events__lambda__stepfunctions.deep-sfn-3-http-task-deauthorized-connection-rejected events__lambda__stepfunctions.deep-sfn-3-http-task-unknown-connection-rejected
Issue-Key: events__lambda__stepfunctions.deep-sfn-3-http-task-echo-roundtrip|run#9|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: events__lambda__stepfunctions.deep-sfn-3-http-task-deauthorized-connection-rejected|run#9|AssertionError|assert first["status"] == "SUCCEEDED", (
Issue-Key: events__lambda__stepfunctions.deep-sfn-3-http-task-unknown-connection-rejected|run#11|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# HTTP Task (arn:aws:states:::http:invoke) is never executed: every execution containing one fails immediately with States.Runtime, so neither the authorized round-trip nor the connection-rejection negatives can be observed

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a state machine whose Task state uses the HTTP Task resource is accepted at CreateStateMachine, but at run time the interpreter refuses to execute the Task state at all.
AWS: Smithy model — StartExecution returns an executionArn and the execution then runs the definition; DescribeExecution.status reaches SUCCEEDED for a definition whose HTTP Task invokes a reachable endpoint through an AUTHORIZED EventBridge connection, and the output carries the endpoint's response. When the connection is deauthorized or names no connection, the recorded behavior is a FAILED execution whose error/cause names the connection problem (States.Http* / task failure), not a generic interpreter error. The model itself says nothing about Task resource semantics, so the success/failure shape here is recorded behavior only.
Emulator: every such execution goes straight to status FAILED with error='States.Runtime' and cause="state 'Call' is of type 'Task', which this emulator does not execute, so nothing was run for it". Because the positive twin never succeeds, the two negative checks cannot distinguish a connection rejection from the interpreter stub either.
Why: a client building any HTTP-Task workflow sees every execution fail with an error name that does not exist in ASL's taxonomy for this condition; retry/catch logic keyed on States.Http errors or on SUCCEEDED output never runs, and the authorization semantics of EventBridge connections are untestable.
Fix: the Step Functions interpreter's Task-state dispatch, specifically the http:invoke resource handler and its connection-authorization lookup.
Traces: events__lambda__sfn.deep-sfn-3-http-task-echo-roundtrip, events__lambda__sfn.deep-sfn-3-http-task-deauthorized-connection-rejected, events__lambda__sfn.deep-sfn-3-http-task-unknown-connection-rejected

Related open issues that do NOT carry these checks' replay keys: 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md and 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md describe the same interpreter gap for other resource types; the HTTP Task resource and its EventBridge-connection authorization path are additional work not listed there.

## Evidence
deep-sfn-3-http-task-echo-roundtrip: AssertionError: the HTTP Task execution must SUCCEED against a live echo endpoint; got 'FAILED' error='States.Runtime' cause="state 'Call' is of type 'Task', which this emulator does not execute, so nothing was run for it"

deep-sfn-3-http-task-deauthorized-connection-rejected: AssertionError: run 1 (connection AUTHORIZED) must SUCCEED; got 'FAILED' error='States.Runtime' cause="state 'Call' is of type 'Task', ... nothing was run for it"

deep-sfn-3-http-task-unknown-connection-rejected: AssertionError: the positive twin (real connection) must SUCCEED before the negative means anything; got 'FAILED' error='States.Runtime' cause="state 'Call' is of type 'Task', ... nothing was run for it"

## Steps to Reproduce
1. Create an EventBridge connection (AUTHORIZED) and an HTTP endpoint that echoes its request body.
2. CreateStateMachine with a single Task state named 'Call' whose Resource is arn:aws:states:::http:invoke, with Parameters ApiEndpoint/Method/Authentication.ConnectionArn pointing at that connection.
3. StartExecution and poll DescribeExecution: expected status SUCCEEDED with the echoed response in output; emulator returns FAILED with error States.Runtime and the 'does not execute Task states' cause.
4. Repeat with the connection deauthorized and with a nonexistent connection ARN: expected a connection-specific failure, emulator returns the same States.Runtime.
