Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-no-match-without-default-rejected stepfunctions.create-state-machine-insufficient-role-permissions-denied stepfunctions.deep-sfn-1-sfn-sync-lambda-function-timeout-is-sandbox-timedout-not-states-timeout stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause stepfunctions.deploy-sfn-1-type-change-replaces-state-machine stepfunctions.describe-execution-kms-access-denied stepfunctions.list-executions-redrive-filter-unsupported-for-express stepfunctions.log-level-selects-which-history-events-reach-cloudwatch-logs stepfunctions.log-sfn-apigw-agreement-all-level-log-events-mirror-execution-history-event-for-event stepfunctions.log-sfn-apigw-agreement-fail-state-error-and-cause-agree-across-log-history-and-describe stepfunctions.log-sfn-apigw-agreement-logging-off-by-update-blocks-delivery-until-reenabled stepfunctions.log-sfn-apigw-agreement-redacted-log-keeps-transitions-and-errors-while-history-keeps-payload stepfunctions.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged stepfunctions.stop-execution-on-express-unsupported
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-no-match-without-default-rejected|run#11|AssertionError|assert _NO_MATCH_CAUSE in no_match_cause, (
Issue-Key: stepfunctions.create-state-machine-insufficient-role-permissions-denied|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-lambda-function-timeout-is-sandbox-timedout-not-states-timeout|run#9|AssertionError|assert len(task_failed) == 1, f"expected exactly one TaskFailed: {types!r}"
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause|run#15|AssertionError|assert set(cause) == {"errorType", "errorMessage", "trace"}, (
Issue-Key: stepfunctions.deploy-sfn-1-type-change-replaces-state-machine|run#24|AssertionError|assert status == "UPDATE_COMPLETE", (
Issue-Key: sfn|StartExecution|KmsAccessDeniedException
Issue-Key: stepfunctions.list-executions-redrive-filter-unsupported-for-express|_error#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: stepfunctions.log-level-selects-which-history-events-reach-cloudwatch-logs|run#13|AssertionError|assert marker in json.dumps(started), (
Issue-Key: stepfunctions.log-sfn-apigw-agreement-all-level-log-events-mirror-execution-history-event-for-event|_assert_mirrors#19|AssertionError|assert got == want, f"{who}: log event {ev['id']} disagrees with history: {got!r} != {want!r}"
Issue-Key: stepfunctions.log-sfn-apigw-agreement-fail-state-error-and-cause-agree-across-log-history-and-describe|_assert_mirrors#11|AssertionError|assert got == want, f"{who}: log event {ev['id']} disagrees with history: {got!r} != {want!r}"
Issue-Key: stepfunctions.log-sfn-apigw-agreement-logging-off-by-update-blocks-delivery-until-reenabled|_assert_mirrors#7|AssertionError|assert got == want, f"{who}: log event {ev['id']} disagrees with history: {got!r} != {want!r}"
Issue-Key: stepfunctions.log-sfn-apigw-agreement-redacted-log-keeps-transitions-and-errors-while-history-keeps-payload|_assert_mirrors#19|AssertionError|assert got == want, f"{who}: log event {ev['id']} disagrees with history: {got!r} != {want!r}"
Issue-Key: stepfunctions.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged|_assert_mirrors#14|AssertionError|assert got == want, f"{who}: log event {ev['id']} disagrees with history: {got!r} != {want!r}"
Issue-Key: stepfunctions.stop-execution-on-express-unsupported|run#16|AssertionError|assert message == expected_stop, f"expected InvalidArn message {expected_stop!r}, got {message!r}"
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# stepfunctions: 10 net-new behavioral causes — Choice no-match cause text, logging-role AccessDenied never raised, Lambda function-timeout classified States.Timeout, extra requestId in Lambda Cause, CFN in-place update NotImplemented, KMS key-policy grant ignored, redriveFilter+stateMachineArn accepted, vended log events miss details and redrive_count, StopExecution express InvalidArn text

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Ten net-new root causes in the Step Functions interpreter, control plane and vended-log serializer, all grounded in failing behavioral checks of the `stepfunctions` suite. None of them is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 10 — a Choice state that matches no rule and declares no Default reports an emulator-authored cause.
AWS: Smithy: a Choice state supports neither Retry nor Catch, so the only signal is the execution's error/cause pair; AWS fails the execution with error `States.Runtime` and a cause whose fixed text is "An error occurred while executing the state '<state name>' (entered at the event id #<id>). Failed to transition out of the state. The state does not point to a next state." — the state name and the entered-at event id are the only variable parts. The same text appears in the ExecutionFailed history event's executionFailedEventDetails.cause (recorded behavior for the exact sentence).
Emulator: error is `States.Runtime`, but the cause is "no choice rule matched the state's input and the state declares no 'Default'" — it neither names the stuck state nor carries the "Failed to transition out of the state. The state does not point to a next state." sentence, so it is byte-indistinguishable in kind from the emulator's other States.Runtime causes (notably the unresolvable-condition-path failure, which must read "The choice state's condition path references an invalid value.").
Why: a client that classifies a States.Runtime failure by its cause text cannot tell an unmatched Choice from any other runtime failure, and cannot report which state got stuck.
Fix: the Choice state's evaluation in the interpreter — the no-match failure's cause construction (and keeping it distinct from the invalid-path cause).
Traces: stepfunctions.choice-no-match-without-default-rejected

Cause 2 of 10 — CreateStateMachine with a logging configuration and a role that lacks the CloudWatch Logs delivery permissions is accepted.
AWS: documented behavior (Step Functions dev guide, cw-logs.html IAM policy); Smithy: CreateStateMachine models InvalidLoggingConfiguration and the service-common refusals, and AWS answers HTTP 400 `AccessDeniedException` with the message "The state machine IAM Role is not authorized to access the Log Destination" (that whole sentence is fixed text, no variable parts) when the roleArn cannot create/describe log deliveries and put log events. The state machine is not created. With the documented logs policy attached, the identical call succeeds and DescribeStateMachine reports status ACTIVE.
Emulator: the call succeeds (returns a stateMachineArn) for a role whose only policy grants s3:ListAllMyBuckets, so no refusal is raised at all.
Why: a client's negative path — "my role is missing log-delivery permissions" — never fires, and a state machine that can never deliver logs is reported as healthy.
Fix: CreateStateMachine's validation: when loggingConfiguration is present, authorize the supplied role for the log destination before creating the resource.
Traces: stepfunctions.create-state-machine-insufficient-role-permissions-denied

Cause 3 of 10 — a Lambda task whose function overruns its own function timeout is reported as a state timeout (TaskTimedOut) and retried.
AWS: recorded behavior: a `arn:aws:states:::lambda:invoke` task whose function exceeds the function's own Timeout fails with TaskFailed carrying error `Sandbox.Timedout` and a JSON cause whose errorType is `Sandbox.Timedout` and whose errorMessage contains "Task timed out after <timeout>.00 seconds" (the number is the function's configured timeout). Because the error name is Sandbox.Timedout and not States.Timeout, a Retry block whose ErrorEquals is ["States.Timeout"] does not match: the function is scheduled exactly once, no TaskTimedOut event is emitted, and ExecutionFailed names Sandbox.Timedout.
Emulator: the history is ExecutionStarted, TaskStateEntered, TaskScheduled, TaskStarted, TaskTimedOut, TaskScheduled, TaskStarted, TaskTimedOut, TaskScheduled, TaskStarted, TaskTimedOut, ExecutionFailed — zero TaskFailed events, three TaskScheduled, and the States.Timeout Retry matched twice.
Why: a client's retry policy keyed on States.Timeout silently triples the invocations of a function that is simply too slow, and no error name identifies the sandbox timeout.
Fix: the lambda:invoke task integration's error classification in the interpreter — a function-level timeout must surface as a TaskFailed/Sandbox.Timedout, not as the state's own TaskTimedOut.
Traces: stepfunctions.deep-sfn-1-sfn-sync-lambda-function-timeout-is-sandbox-timedout-not-states-timeout

Cause 4 of 10 — the Cause object of a Lambda task that raised an unhandled exception carries an extra requestId member.
AWS: recorded behavior: for an unhandled handler exception the TaskFailed cause is a JSON document with exactly the keys errorType, errorMessage and trace (errorType is the thrown class name, errorMessage the thrown message, trace the stack frames). No requestId member is present.
Emulator: the cause object has keys errorMessage, errorType, requestId and trace.
Why: a client that round-trips or schema-validates the cause document (or diffs it against a golden) sees an unexpected member, and code that forwards the cause as the error payload leaks a field AWS does not define there.
Fix: the lambda:invoke task integration's failure serializer — the cause document's member set.
Traces: stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause

Cause 5 of 10 — a CloudFormation stack UPDATE that changes an AWS::StepFunctions::StateMachine's properties is refused as NotImplemented, so the stack rolls back.
AWS: recorded behavior: changing the state machine's Type (STANDARD<->EXPRESS) is a replacement update — CloudFormation creates the replacement, updates the logical resource's physical id and the stack reaches UPDATE_COMPLETE; a property change that is not a replacement is applied in place. Either way the stack reaches UPDATE_COMPLETE.
Emulator: the stack goes UPDATE_FAILED -> UPDATE_ROLLBACK_IN_PROGRESS with the resource event "NotImplemented: this emulator does not change the properties of an existing AWS::StepFunctions::StateMachine in place; the resource was left as it is."
Why: no deployed Step Functions workflow can ever be changed through its stack — every template edit rolls the stack back, and the status/reason wording is emulator-authored, not CloudFormation's.
Fix: the AWS::StepFunctions::StateMachine resource provider's update path (in-place update plus replacement when an immutable property such as Type changes).
Traces: stepfunctions.deploy-sfn-1-type-change-replaces-state-machine

Cause 6 of 10 — a KMS key policy that grants the state machine's execution role crypto access is ignored, so StartExecution on a CUSTOMER_MANAGED_KMS_KEY state machine is refused.
AWS: Smithy: StartExecution models KmsAccessDeniedException (HTTP 400), documented as "Either your KMS key policy or API caller does not have the required permissions" — i.e. the key's RESOURCE-based policy is one of the two grants that authorize the call. With a key policy that explicitly allows the states execution role kms:GenerateDataKey/kms:Decrypt, StartExecution succeeds and the execution reaches RUNNING; the refusal only appears once that grant is revoked, and then it relays KMS's own message ("User: <role arn> is not authorized to perform: kms:Decrypt on resource: <key arn> with an explicit deny in a resource-based policy ...").
Emulator: StartExecution is refused with KmsAccessDeniedException whose message reads "User: <role arn> is not authorized to perform: kms:GenerateDataKey on resource: <key arn> because no identity-based policy allows the kms:GenerateDataKey action" even though the key policy explicitly allows that role — only identity-based policy is evaluated, the key policy grant is not.
Why: a CMK-encrypted state machine cannot be started at all when access is granted the documented way (key policy), and the scenario under test — a read refused only after the grant is revoked — can never be set up.
Fix: the KMS authorization used by the Step Functions encryption path must evaluate the key's resource-based policy alongside identity policy (StartExecution/DescribeExecution data-key negotiation).
Traces: stepfunctions.describe-execution-kms-access-denied

Cause 7 of 10 — ListExecutions accepts redriveFilter together with stateMachineArn instead of refusing it.
AWS: Smithy: ListExecutions.redriveFilter is an ExecutionRedriveFilter enum (REDRIVEN|NOT_REDRIVEN) and its own documentation states "If you provide a state machine ARN in redriveFilter, the API returns a validation exception"; ListExecutions models ValidationException (HTTP 400). The message is "RedriveFilter cannot be applied to ListExecutions call with StateMachine ARN" (fixed text, no variable parts), and the refusal is identical for STANDARD and EXPRESS state machines because redriveFilter requires a mapRunArn.
Emulator: the call answers HTTP 200 with {"executions": []} for both the STANDARD and the EXPRESS state machine.
Why: a client that mistakenly combines the two parameters gets a silently empty, authoritative-looking page instead of the validation error, and reads "no redriven executions" from a request AWS never answers.
Fix: ListExecutions' input validation — reject redriveFilter when stateMachineArn is supplied, before any listing.
Traces: stepfunctions.list-executions-redrive-filter-unsupported-for-express

Cause 8 of 10 — vended CloudWatch Logs events carry no `details` object, so includeExecutionData=true delivers no payload.
AWS: recorded behavior (cw-logs.html): a vended execution-history log event is {"id","type","details":{...},"previous_event_id","event_timestamp","execution_arn","redrive_count"}; with level=ALL and includeExecutionData=true the `details` object mirrors the history event's details — for ExecutionStarted that is input, inputDetails and roleArn, so the execution input appears in the log event. With includeExecutionData=false the same events are delivered with the payload members (input, inputDetails, output, outputDetails, parameters) stripped, and `details` still present for the non-payload members.
Emulator: the ExecutionStarted log event is {"id":"1","type":"ExecutionStarted","previous_event_id":"0","event_timestamp":"<ms>","execution_arn":"<arn>"} — no `details` member at all, so the execution input never appears and includeExecutionData has no observable effect.
Why: CloudWatch Logs is the only record of an execution's payload for EXPRESS and the agreed record for STANDARD; a client reading input/output from the log event finds nothing, and the redaction contract (true vs false) cannot be observed.
Fix: the vended-log event serializer for execution history events — populate `details` from the history event and apply includeExecutionData as a payload filter.
Traces: stepfunctions.log-level-selects-which-history-events-reach-cloudwatch-logs

Cause 9 of 10 — vended CloudWatch Logs events omit `redrive_count`.
AWS: recorded behavior: every vended execution log event carries `redrive_count` as a STRING, "0" for an execution that has never been redriven (and the redrive count thereafter). It is present on every event type, including ExecutionStarted.
Emulator: the member is absent from the delivered event (reads as None), so log events disagree with the execution history they must mirror event for event.
Why: a consumer that joins log events to history events on the full member set, or that reads redrive_count to tell a redriven run from a first run, has no value to read.
Fix: the vended-log event serializer — emit redrive_count on every event, defaulting to "0".
Traces: stepfunctions.log-sfn-apigw-agreement-all-level-log-events-mirror-execution-history-event-for-event, stepfunctions.log-sfn-apigw-agreement-fail-state-error-and-cause-agree-across-log-history-and-describe, stepfunctions.log-sfn-apigw-agreement-logging-off-by-update-blocks-delivery-until-reenabled, stepfunctions.log-sfn-apigw-agreement-redacted-log-keeps-transitions-and-errors-while-history-keeps-payload, stepfunctions.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged

Cause 10 of 10 — StopExecution's InvalidArn message for an EXPRESS execution is emulator-authored.
AWS: Smithy: StopExecution models InvalidArn (HTTP 400, "The provided Amazon Resource Name (ARN) is not valid"). AWS refuses the ARN by its RESOURCE TYPE, not by the execution: the message is "Invalid Arn: 'Resource type not valid in this context: express'" — fixed text "Invalid Arn: 'Resource type not valid in this context: " + the offending resource-type segment of the ARN + "'". Nothing else (no operation name, no ARN) appears.
Emulator: HTTP 400 InvalidArn with the message "Invalid Arn: 'StopExecution is not supported by EXPRESS state machines: <the execution arn>'" — the operation name and the whole ARN instead of the resource type.
Why: a client matching or redacting on AWS's wording does not match, and the message does not tell it which ARN segment was rejected. Note the related OPEN issue 20261103T052008Z-states-express-executions-get-a-standard-execution-arn-so.md: it carries the same resource-type refusal for DescribeExecution, GetExecutionHistory and RedriveExecution and the express-ARN minting itself, but NOT StopExecution and not this behavioral check, so this cause is not covered by it.
Fix: StopExecution's ARN validation must raise the shared InvalidArn resource-type refusal rather than an operation-specific sentence.
Traces: stepfunctions.stop-execution-on-express-unsupported

## Evidence
All ten causes are failures from this replay of simparity-behavioral-aws-v1 against the emulator built from this checkout.

Cause 1: AssertionError: the cause must say the state could not transition, so a caller can tell an unmatched Choice apart from any other States.Runtime failure; got "no choice rule matched the state's input and the state declares no 'Default'". Expected the cause to contain "Failed to transition out of the state. The state does not point to a next state." and to name the state ('Route').

Cause 2: AssertionError: expected a ClientError, but the call succeeded: None — CreateStateMachine(loggingConfiguration=<log group>, roleArn=<role with only s3:ListAllMyBuckets>) returned 200. Expected code AccessDeniedException, message "The state machine IAM Role is not authorized to access the Log Destination", and the name absent from ListStateMachines.

Cause 3: AssertionError: expected exactly one TaskFailed: ['ExecutionStarted', 'TaskStateEntered', 'TaskScheduled', 'TaskStarted', 'TaskTimedOut', 'TaskScheduled', 'TaskStarted', 'TaskTimedOut', 'TaskScheduled', 'TaskStarted', 'TaskTimedOut', 'ExecutionFailed']. Expected one TaskScheduled, no TaskTimedOut, TaskFailed.error == 'Sandbox.Timedout', cause.errorMessage containing 'Task timed out after 2.00 seconds', ExecutionFailed.error == 'Sandbox.Timedout'.

Cause 4: AssertionError: the Cause object has keys ['errorMessage', 'errorType', 'requestId', 'trace'], expected errorType/errorMessage/trace.

Cause 5: AssertionError: stack update ended 'UPDATE_ROLLBACK_IN_PROGRESS', expected 'UPDATE_COMPLETE'; failed events: [(<stack>, 'UPDATE_FAILED', 'The following resource(s) failed to update: [Workflow]. '), ('Workflow', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::StepFunctions::StateMachine in place; the resource was left as it is.')].

Cause 6: botocore.errorfactory.KmsAccessDeniedException on the StartExecution call that sets the scenario up: "User: <states execution role arn> is not authorized to perform: kms:GenerateDataKey on resource: <cmk arn> because no identity-based policy allows the kms:GenerateDataKey action (Service: Kms; Status Code: 400; Error Code: AccessDeniedException ...)" — raised although the check had just set a key policy explicitly allowing that role kms:GenerateDataKey/kms:Decrypt. Expected StartExecution to succeed (execution RUNNING), and only the post-revoke DescribeExecution to be refused with KmsAccessDeniedException/400.

Cause 7: AssertionError: expected a ClientError, but the call succeeded: {'executions': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for ListExecutions(stateMachineArn=<arn>, redriveFilter='NOT_REDRIVEN') on both a STANDARD and an EXPRESS state machine. Expected ValidationException / 400 / "RedriveFilter cannot be applied to ListExecutions call with StateMachine ARN".

Cause 8: AssertionError: level=ALL with includeExecutionData=true must include the execution input, so the marker '<marker>' must appear in the ExecutionStarted event; the event was {"id": "1", "type": "ExecutionStarted", "previous_event_id": "0", "event_timestamp": "<ms>", "execution_arn": "<arn>"} — no `details` member.

Cause 9 (all five log-sfn-apigw-agreement checks, identical shape): AssertionError: log event 1 disagrees with history: {... 'redrive_count': None} != {... 'redrive_count': '0'} — every other member of the ExecutionStarted log event agreed.

Cause 10: AssertionError: expected InvalidArn message "Invalid Arn: 'Resource type not valid in this context: express'", got "Invalid Arn: 'StopExecution is not supported by EXPRESS state machines: <the execution arn>'".

## Steps to Reproduce
Replay the simparity-behavioral-aws-v1 checks listed in trace_ids against the emulator (run_behavioral_suite), then per cause:
1. Cause 1: create a STANDARD state machine whose StartAt is a Choice with one rule and no Default; start it with an input matching no rule; read DescribeExecution error/cause and the ExecutionFailed history event. Repeat with an input whose condition path does not resolve and compare the two causes.
2. Cause 2: create a log group, create an IAM role trusted by states.amazonaws.com whose only policy is s3:ListAllMyBuckets, then CreateStateMachine with loggingConfiguration pointing at the group and that roleArn.
3. Cause 3: deploy a python3.12 function with Timeout=2 that sleeps 5; invoke it from a Task (arn:aws:states:::lambda:invoke) with Retry[ErrorEquals=[States.Timeout], MaxAttempts=2]; read GetExecutionHistory.
4. Cause 4: invoke a handler that throws a custom Error from the same task shape and JSON-parse the TaskFailed cause.
5. Cause 5: create a stack with an AWS::StepFunctions::StateMachine, then UpdateStack with the Type changed, and poll the stack status and events.
6. Cause 6: create a CMK, set a key policy allowing a states execution role kms:GenerateDataKey/kms:Decrypt, create a state machine with encryptionConfiguration CUSTOMER_MANAGED_KMS_KEY, then StartExecution.
7. Cause 7: ListExecutions(stateMachineArn=<any>, redriveFilter='NOT_REDRIVEN') on a STANDARD and on an EXPRESS state machine.
8. Cause 8: create a state machine with loggingConfiguration level=ALL, includeExecutionData=true, start an execution with a marked input, then read the log group's events.
9. Cause 9: same as 8 and compare every member of each log event against GetExecutionHistory's corresponding event.
10. Cause 10: start an execution on an EXPRESS state machine and call StopExecution on the returned ARN; compare the InvalidArn message.
