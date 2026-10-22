Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-start-execution-idempotent-while-running
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-start-execution-idempotent-while-running sfn-state-machine-deleting-rejections states-creation-input-validation-errors states-execution-lifecycle-success-and-abort states-standard-execution-lifecycle stepfunctions-standard-execution-start-monitor-stop sfn-state-machine-create-idempotent-repeat stepfunctions-state-machine-crud-and-tagging states-state-machine-lifecycle states-state-machine-lifecycle-crud states-state-machine-versioning-and-aliases states-express-state-machine-type-restrictions stepfunctions-express-sync-execution-type-restrictions stepfunctions-map-run-error-paths
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|CreateStateMachine|4xx->4xx|$.message
Issue-Key: states|ListExecutions|4xx->4xx|$.message
Issue-Key: states|UpdateMapRun|4xx->4xx|$.message
Issue-Related: 20261022T120501Z-states-malformed-cross-region-and-labeled-arns-answered-as.md
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Step Functions error messages do not follow the AWS message rules

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
The HTTP status and error code are correct in all five clusters below; only the message text diverges, and in each case the AWS text follows a rule that is known (a fixed service string, a '<Condition>: <quoted ARN>' template built from the request, or the standard validation-error template rendering the member's modeled type). The emulator substitutes its own prose instead. 5 causes.

Cause 1 of 5: ExecutionAlreadyExists message.
AWS: Smithy: StartExecution error ExecutionAlreadyExists (client, HTTP 400), 'The execution has the same name as another execution (but a different input)'. The message is built from the request as "Execution Already Exists: '<executionArn>'" - the condition name followed by the colliding ARN in single quotes.
Emulator: 400 ExecutionAlreadyExists with 'execution <arn> already exists'.
Why: tooling that parses or displays the AWS message (and tests that match on the 'Execution Already Exists:' prefix) diverges even though the error code matches.
Fix: the StartExecution handler's error message construction.
Traces: sfn-start-execution-idempotent-while-running#task#5 (also sfn-start-execution-idempotent-while-running#task#6/#11, sfn-state-machine-deleting-rejections#task#7, states-creation-input-validation-errors#task#10/#11, states-execution-lifecycle-success-and-abort#task#9, states-standard-execution-lifecycle#task#9, stepfunctions-standard-execution-start-monitor-stop#task#8).

Cause 2 of 5: StateMachineAlreadyExists message.
AWS: Smithy: CreateStateMachine error StateMachineAlreadyExists (client, HTTP 400), 'A state machine with the same name but a different definition or role ARN already exists'. Message rule as above: "State Machine Already Exists: '<stateMachineArn>'".
Emulator: "a state machine named '<name>' already exists with a different definition or configuration" - a different template, naming the name rather than the ARN.
Fix: the CreateStateMachine conflict path's message construction.
Traces: sfn-state-machine-create-idempotent-repeat#task#5 (also sfn-state-machine-deleting-rejections#task#6, states-creation-input-validation-errors#task#8/#9, stepfunctions-state-machine-crud-and-tagging#task#12).

Cause 3 of 5: StateMachineDoesNotExist message.
AWS: Smithy: DescribeStateMachine error StateMachineDoesNotExist (client, HTTP 400). Message rule: "State Machine Does Not Exist: '<stateMachineArn>'".
Emulator: 'state machine <arn> does not exist'.
Fix: the shared state-machine-lookup error message construction.
Traces: states-standard-execution-lifecycle#teardown#14 (also states-state-machine-lifecycle#teardown#9, states-state-machine-lifecycle-crud#task#10, states-state-machine-versioning-and-aliases#teardown#20, stepfunctions-state-machine-crud-and-tagging#task#14/#teardown#16).

Cause 4 of 5: StateMachineTypeNotSupported message is rebuilt from request values.
AWS: Smithy: ListExecutions error StateMachineTypeNotSupported (client, HTTP 400), 'State machine type is not supported'. The recorded message is the fixed service string 'This operation is not supported by this type of state machine' - it carries no request values.
Emulator: 'ListExecutions does not support the EXPRESS state machine <arn>' - a message synthesised from the operation name, type and ARN.
Why: AWS's text here is a constant; echoing request values into it means the emulator's message varies with the request where AWS's does not, so assertions on the constant fail.
Fix: the type-restriction check in the ListExecutions handler - return the fixed service text.
Traces: states-express-state-machine-type-restrictions#task#7 (also stepfunctions-express-sync-execution-type-restrictions#task#6).

Cause 5 of 5: validation-error message renders a float member as an integer.
AWS: Smithy: UpdateMapRun input toleratedFailurePercentage -> ToleratedFailurePercentage, type float, range min 0 max 100; error ValidationException (client, HTTP 400). The standard validation template renders the rejected value in the member's modeled type, so a float member renders as '150.0': "1 validation error detected: Value '150.0' at 'toleratedFailurePercentage' failed to satisfy constraint: Member must have value less than or equal to 100".
Emulator: the identical template but with the value rendered as '150' - the float is stringified as an integer.
Why: clients and tests that compare the validation message (the only place the rejected value is reported) see a different value rendering than AWS produces for a float member.
Fix: the validation-error message formatter in the sfn service - render the rejected value according to the member's modeled type.
Traces: stepfunctions-map-run-error-paths#task#8.

## Evidence
All five clusters: status and error code match; only $.message differs.
- sfn-start-execution-idempotent-while-running#task#5 (400): expected "Execution Already Exists: 'arn:aws:states:us-west-2:000000000000:execution:...:sim2real-probe-exec-1'"; actual 'execution arn:... already exists' (9 affected steps).
- sfn-state-machine-create-idempotent-repeat#task#5 (400): expected "State Machine Already Exists: 'arn:...:stateMachine:sim2real-probe-sfn-idem-...'"; actual "a state machine named '...' already exists with a different definition or configuration" (5 affected).
- states-standard-execution-lifecycle#teardown#14 (400): expected "State Machine Does Not Exist: 'arn:...:stateMachine:sim2real-probe-stepfunctions-sm-exec-52a8c544'"; actual 'state machine arn:... does not exist' (6 affected).
- states-express-state-machine-type-restrictions#task#7 (400): expected 'This operation is not supported by this type of state machine'; actual 'ListExecutions does not support the EXPRESS state machine arn:...' (2 affected).
- stepfunctions-map-run-error-paths#task#8 (400): expected "Value '150.0' at 'toleratedFailurePercentage'..."; actual "Value '150' at 'toleratedFailurePercentage'...".

## Steps to Reproduce
1. Replay sfn-start-execution-idempotent-while-running step task#5 (StartExecution with a name already running) and sfn-state-machine-create-idempotent-repeat step task#5 (CreateStateMachine with an existing name and a different definition).
2. Replay states-standard-execution-lifecycle step teardown#14 (DescribeStateMachine after delete).
3. Replay states-express-state-machine-type-restrictions step task#7 (ListExecutions on an EXPRESS state machine).
4. Replay stepfunctions-map-run-error-paths step task#8 (UpdateMapRun with toleratedFailurePercentage=150).
