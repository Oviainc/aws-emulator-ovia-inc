Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-state-machine-deleting-rejections
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-state-machine-deleting-rejections states-state-machine-lifecycle-crud stepfunctions-state-machine-crud-and-tagging states-creation-input-validation-errors states-execution-failure-and-redrive stepfunctions-redrive-execution-error-paths stepfunctions-test-state-in-isolation
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|UpdateStateMachine|4xx->4xx|$.message
Issue-Key: states|StartExecution|4xx->4xx|$.message
Issue-Key: states|RedriveExecution|4xx->4xx|$.message
Issue-Key: states|TestState|4xx->4xx|$.message
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# states client-error messages omit AWS's ARN-prefixed wrappers and enum ordering

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStateMachine

## Description
Four client-error paths return the correct HTTP status (400) and the correct modeled error, but build the `message` member by a different rule than AWS: the request-derived wrapper AWS puts around the detail is missing or reworded. In each cause below the differing text is produced by a rule that is known (it is built from the request - the state-machine or execution ARN, or the member name and its enum constraint), so the construction is the requirement; the recorded sentences are quoted as examples of that construction. 4 causes.

Cause 1 of 4: StateMachineDeleting on UpdateStateMachine words the refusal differently.
AWS: Smithy: UpdateStateMachine error StateMachineDeleting, errorKind client, httpError 400, member message -> ErrorMessage; shape doc 'The specified state machine is being deleted.' Recorded behavior: the message is built from the request as 'State Machine is being deleted: '<stateMachineArn>'' (example: "State Machine is being deleted: 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-probe-sfn-del-...'").
Emulator: status 400 and the same error shape, but the message prefix is 'State Machine Deleting:' instead of 'State Machine is being deleted:' (the ARN is interpolated correctly).
Why: callers and support tooling that match on the AWS refusal sentence while a delete is in flight do not recognise the emulator's wording, so a retry-until-deleted loop keyed on the message does not fire.
Fix: the StateMachineDeleting message construction in the sfn service's error rendering (shared by CreateStateMachine/StartExecution/UpdateStateMachine), not the handler's precondition - the precondition itself fires correctly.
Traces: sfn-state-machine-deleting-rejections#task#8 (cluster also covers states-state-machine-lifecycle-crud#task#8 and stepfunctions-state-machine-crud-and-tagging#task#13).

Cause 2 of 4: InvalidExecutionInput on StartExecution drops AWS's wrapper entirely.
AWS: Smithy: StartExecution error InvalidExecutionInput, errorKind client, httpError 400, doc 'The provided JSON input data is not valid.' Recorded behavior: the message wraps the parser detail as "Invalid State Machine Execution Input: '<detail>'" (example detail: "Unexpected character ('n' (code 110)): was expecting double-quote to start field name").
Emulator: status 400 with the right error, but the message is the bare internal parser string 'input is not valid JSON: key must be a string at line 1 column 2' - no 'Invalid State Machine Execution Input:' prefix and no quoting of the detail. (Contrast CreateStateMachine, where the emulator does emit the analogous "Invalid State Machine Definition: '...'" wrapper, so the wrapper rule is already implemented elsewhere in the service.)
Why: a client that distinguishes a bad execution input from other 400s by the message prefix - and any log/alarm pattern built on it - cannot classify the emulator's response; the error surface also leaks the emulator's own parser vocabulary.
Fix: the StartExecution input-validation error path in the sfn service: wrap the JSON-parse detail in the 'Invalid State Machine Execution Input: ...' form the service already uses for definitions.
Traces: states-creation-input-validation-errors#task#11.

Cause 3 of 4: ExecutionNotRedrivable on RedriveExecution omits the 'Execution <arn> invalid for redrive as:' wrapper.
AWS: Smithy: RedriveExecution error ExecutionNotRedrivable, errorKind client, httpError 400, doc 'The execution Amazon Resource Name (ARN) that you specified for executionArn cannot be redriven.' Recorded behavior: the message is built from the request as 'Execution <executionArn> invalid for redrive as: <reason>' (example reason: 'Execution is SUCCEEDED and cannot be redriven').
Emulator: status 400 with the right error, but the message is only the reason ('Execution is SUCCEEDED and cannot be redriven') - the 'Execution <arn> invalid for redrive as: ' prefix naming the offending execution is absent.
Why: the AWS message identifies WHICH execution was refused, which is what a caller redriving several executions needs to attribute the failure; the emulator's message is unattributable.
Fix: the RedriveExecution refusal path in the sfn service: compose the message from the request's executionArn plus the reason.
Traces: states-execution-failure-and-redrive#task#15 (cluster also covers stepfunctions-redrive-execution-error-paths#task#5).

Cause 4 of 4: TestState's enum ValidationException lists the inspectionLevel value set in a different order than AWS.
AWS: Smithy: TestState input member inspectionLevel -> InspectionLevel, error ValidationException (client, httpError 400). Recorded behavior: AWS's validation message names the member and its constraint and enumerates the permitted values as "1 validation error detected: Value 'NOT_A_REAL_LEVEL' at 'inspectionLevel' failed to satisfy constraint: Member must satisfy enum value set: [TRACE, INFO, DEBUG]".
Emulator: status 400 and the same ValidationException and the same sentence form, but the value set is rendered in the model's declaration order, '[INFO, DEBUG, TRACE]' (Smithy: InspectionLevel values [INFO, DEBUG, TRACE]) rather than AWS's recorded order [TRACE, INFO, DEBUG].
Why: constraint-violation messages of this exact form are what clients and test suites assert on for enum members; the set is right but the rendering differs, so a byte-comparison of the AWS validation message fails. This is the lowest-impact cause in the bundle - status and error code already match.
Fix: the enum-validation message rendering for TestState.inspectionLevel in the sfn service's input validation (the order AWS emits is fixed per member, not the Smithy declaration order).
Traces: stepfunctions-test-state-in-isolation#task#4.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, state_reset_failures 0). All four clusters are verdict=mismatch with expected status 400 and actual status 400 - the status and error shape match, only `$.message` diverges:
- sfn-state-machine-deleting-rejections#task#8 UpdateStateMachine: expected "State Machine is being deleted: '<arn>'", got "State Machine Deleting: '<arn>'" (same ARN interpolated). affected_count 3.
- states-creation-input-validation-errors#task#11 StartExecution: expected "Invalid State Machine Execution Input: 'Unexpected character ('n' (code 110)): was expecting double-quote to start field name'", got 'input is not valid JSON: key must be a string at line 1 column 2' (wrapper absent). affected_count 1.
- states-execution-failure-and-redrive#task#15 RedriveExecution: expected 'Execution <executionArn> invalid for redrive as: Execution is SUCCEEDED and cannot be redriven', got 'Execution is SUCCEEDED and cannot be redriven' (ARN wrapper absent). affected_count 2.
- stepfunctions-test-state-in-isolation#task#4 TestState: expected "...Member must satisfy enum value set: [TRACE, INFO, DEBUG]", got "...[INFO, DEBUG, TRACE]". affected_count 1.
Smithy evidence: StateMachineDeleting / InvalidExecutionInput / ExecutionNotRedrivable are all client errors with httpError 400 and a single `message` member; InspectionLevel values are [INFO, DEBUG, TRACE].

## Steps to Reproduce
1. Replay fixture sfn-state-machine-deleting-rejections; step task#8 calls UpdateStateMachine on a state machine being deleted -> 400 StateMachineDeleting whose message begins 'State Machine Deleting:' instead of 'State Machine is being deleted:'.
2. Replay fixture states-creation-input-validation-errors; step task#11 calls StartExecution with a non-JSON input -> 400 InvalidExecutionInput with a bare parser message and no 'Invalid State Machine Execution Input:' wrapper.
3. Replay fixture states-execution-failure-and-redrive; step task#15 calls RedriveExecution on a SUCCEEDED execution -> 400 ExecutionNotRedrivable with no 'Execution <arn> invalid for redrive as: ' prefix.
4. Replay fixture stepfunctions-test-state-in-isolation; step task#4 calls TestState with inspectionLevel='NOT_A_REAL_LEVEL' -> 400 ValidationException listing the enum set as [INFO, DEBUG, TRACE].
5. Re-replay all four steps after the fix; each is a separate cluster and must pass individually.
