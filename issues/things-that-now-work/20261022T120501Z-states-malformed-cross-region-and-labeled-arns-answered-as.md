Issue-Source: trace-sweep-divergence
Issue-Trace: states-standard-execution-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: states-standard-execution-lifecycle states-state-machine-lifecycle states-tagging-operations sfn-validation-reason-labeled-arn-and-completed-map-run states-execution-lifecycle-success-and-abort stepfunctions-standard-execution-start-monitor-stop
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|StartExecution|4xx->4xx|$.__type
Issue-Key: states|StartExecution|4xx->4xx|$.message
Issue-Key: states|DescribeExecution|4xx->4xx|$.__type
Issue-Key: states|DescribeExecution|4xx->4xx|$.message
Issue-Key: states|DescribeStateMachine|4xx->4xx|$.__type
Issue-Key: states|DescribeStateMachine|4xx->4xx|$.message
Issue-Key: states|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: states|ListTagsForResource|4xx->4xx|$.message
Issue-Key: states|ListTagsForResource|4xx->4xx|status
Issue-Key: states|StartExecution|4xx->4xx|$.reason
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Malformed, cross-region and labeled ARNs answered as not-found instead of InvalidArn or ValidationException

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Step Functions validates an ARN before it looks the resource up: a syntactically wrong ARN, an ARN from another region, or an ARN carrying a map-run label is a client ARN/validation error, not a not-found. The emulator skips that stage everywhere and reports the resource as absent instead. 6 causes.

Cause 1 of 6: StartExecution with a cross-region state machine ARN.
AWS: Smithy: StartExecution errors include InvalidArn (client, HTTP 400, 'The provided Amazon Resource Name (ARN) is not valid') and StateMachineDoesNotExist (client, 400). The recording answers 400 with __type 'com.amazonaws.swf.service.v2.model#InvalidArn' and a message built from the request: "Invalid Arn: 'Expected the ARN <arn> to be within region (<endpoint region>).'".
Emulator: 400 with error code StateMachineDoesNotExist and a not-found message naming the ARN.
Why: a client that distinguishes 'wrong ARN' (never retry, fix the input) from 'not found' (maybe create it) takes the wrong branch, and a cross-region ARN is reported as a missing resource in the local region.
Fix: ARN parsing/region validation in front of state machine lookup in the sfn service (shared ARN validator used by the StartExecution handler).
Traces: states-standard-execution-lifecycle#task#10.

Cause 2 of 6: DescribeExecution with a cross-region execution ARN.
AWS: Smithy: DescribeExecution errors InvalidArn and ExecutionDoesNotExist. The recording answers 400 InvalidArn with "Invalid Arn: 'Expected the ARN <arn> to be within region (<endpoint region>).'".
Emulator: 400 with error code ExecutionDoesNotExist and a not-found message.
Why/Fix: as cause 1, for the execution lookup path.
Traces: states-standard-execution-lifecycle#task#11.

Cause 3 of 6: DescribeStateMachine with a cross-region state machine ARN.
AWS: 400 InvalidArn with the same region-mismatch message rule.
Emulator: 400 StateMachineDoesNotExist.
Why/Fix: as cause 1, for the DescribeStateMachine handler.
Traces: states-state-machine-lifecycle#task#6.

Cause 4 of 6: ListTagsForResource with a cross-region resource ARN returns the wrong status as well as the wrong code.
AWS: Smithy: ListTagsForResource errors InvalidArn (client, HTTP 400) and ResourceNotFound (client, HTTP 404). A cross-region ARN is the former: the recording answers HTTP 400 with __type 'com.amazonaws.swf.service.v2.model#InvalidArn'.
Emulator: HTTP 404 with error code ResourceNotFound.
Why: the HTTP status itself diverges, so a client that keys retry/404-handling on status mis-handles a malformed request as a missing resource.
Fix: the same ARN validator, applied in the tagging handlers before resource resolution.
Traces: states-tagging-operations#task#10.

Cause 5 of 6: StartExecution against a labeled (map-run) state machine ARN.
AWS: Smithy: StartExecution errors include ValidationException (client, HTTP 400) whose reason member is ValidationExceptionReason, which enumerates API_DOES_NOT_SUPPORT_LABELED_ARNS. The recording answers 400 with __type 'com.amazonaws.swf.service.v2.model#ValidationException', reason 'API_DOES_NOT_SUPPORT_LABELED_ARNS' and message 'Labeled State Machine ARNs are not supported for this API'.
Emulator: 400 with error code StateMachineDoesNotExist, no reason member, and a not-found message.
Why: the modeled reason code is how clients detect this class of rejection; reporting it as a missing state machine hides a permanent input defect and loses the reason enum entirely.
Fix: ARN parsing in the StartExecution handler (detect the '<arn>/<label>' labeled form and raise ValidationException with reason API_DOES_NOT_SUPPORT_LABELED_ARNS).
Traces: sfn-validation-reason-labeled-arn-and-completed-map-run#task#3.

Cause 6 of 6: DescribeExecution with a syntactically invalid ARN gets InvalidArn but not its message rule.
AWS: the error type is right here; the message is built from the request and states which part of the ARN is wrong: "Invalid Arn: 'Invalid ARN prefix: <value>'" for a value that does not start with 'arn:'.
Emulator: 400 InvalidArn with "'not-a-valid-arn' is not an execution ARN of this service" - no 'Invalid Arn:' envelope and no prefix diagnosis.
Why: the message is the only place AWS reports which ARN component failed, so tooling that surfaces it to users reports something different from AWS.
Fix: the ARN validator's message construction, shared by the execution-ARN handlers.
Traces: states-execution-lifecycle-success-and-abort#task#10 (also stepfunctions-standard-execution-start-monitor-stop#task#9).

## Evidence
- states-standard-execution-lifecycle#task#10 (400 both sides): expected $.__type='com.amazonaws.swf.service.v2.model#InvalidArn', message "Invalid Arn: 'Expected the ARN arn:aws:states:us-east-1:000000000000:stateMachine:nonexistent to be within region (us-west-2).'"; actual error StateMachineDoesNotExist, message 'state machine ... does not exist'.
- states-standard-execution-lifecycle#task#11 (400 both sides): expected InvalidArn; actual ExecutionDoesNotExist.
- states-state-machine-lifecycle#task#6 (400 both sides): expected InvalidArn; actual StateMachineDoesNotExist.
- states-tagging-operations#task#10: expected status 400 with InvalidArn; actual status 404 with ResourceNotFound.
- sfn-validation-reason-labeled-arn-and-completed-map-run#task#3 (400 both sides): expected __type ValidationException, $.reason='API_DOES_NOT_SUPPORT_LABELED_ARNS', message 'Labeled State Machine ARNs are not supported for this API'; actual StateMachineDoesNotExist with no reason member.
- states-execution-lifecycle-success-and-abort#task#10 (400 both sides, correct InvalidArn): expected message "Invalid Arn: 'Invalid ARN prefix: not-a-valid-arn'"; actual "'not-a-valid-arn' is not an execution ARN of this service".

## Steps to Reproduce
1. Replay states-standard-execution-lifecycle steps task#10/task#11: StartExecution and DescribeExecution with us-east-1 ARNs against the us-west-2 endpoint -> emulator answers StateMachineDoesNotExist / ExecutionDoesNotExist instead of InvalidArn.
2. Replay states-state-machine-lifecycle step task#6 (DescribeStateMachine, cross-region ARN) and states-tagging-operations step task#10 (ListTagsForResource, cross-region ARN -> 404 instead of 400 InvalidArn).
3. Replay sfn-validation-reason-labeled-arn-and-completed-map-run step task#3: StartExecution on '<stateMachineArn>/mapLabel' -> expect ValidationException with reason API_DOES_NOT_SUPPORT_LABELED_ARNS.
4. Replay states-execution-lifecycle-success-and-abort step task#10: DescribeExecution with executionArn 'not-a-valid-arn'.
