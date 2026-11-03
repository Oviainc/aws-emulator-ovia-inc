Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure
Issue-Key: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure|_poll_until#10|Timeout|last = probe()
Issue-Commit: ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5

# dynamodb__lambda__stepfunctions: the saga's failing Lambda task never routes through its Catch to the compensating state, so the compensation never runs (1 cause)

> _Note: this issue was identified against the code base at commit `ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause in this service group. The report does not mark this check as owned by any open issue, so it is filed here; open issue 20261102T111517Z-dynamodb-lambda-stepfunctions-a-saga-whose-lambda-task.md describes the same symptom but does not carry this run's replay key, and open issues 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md describe neighbouring causes.

Cause 1 of 1: a STANDARD state machine implementing the saga pattern (optimized dynamodb:putItem reserve step, then an optimized lambda:invoke charge step whose handler raises, with Catch ErrorEquals [States.ALL] routing to a compensating lambda:invoke) never reaches its Catch target, and the check spins on restarting executions until the harness kills it at 60 s. Failing check: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure.

AWS: a Task whose Resource is arn:aws:states:::lambda:invoke runs the function; when the handler raises, the Task fails with the Lambda-derived error name (Lambda.Unhandled, or Lambda.<ExceptionClass>) and a cause that is the function's structured error document ({"errorType","errorMessage","trace"} — values from the raised exception). A Catch entry with ErrorEquals ["States.ALL"] matches that failure, writes the error document to the declared ResultPath ($.error) and transitions to Next, so the compensating Task runs and the execution reaches a terminal status of SUCCEEDED; DescribeExecution then reports status="SUCCEEDED" and the compensating function's side effect (a DynamoDB item with pk="compensation-executed") is readable with GetItem. Smithy (sfn): DescribeExecution models status, error and cause as the carriers of this outcome; Smithy (lambda): Invoke is a modeled data-plane operation that actually runs the function.
Emulator: the check records only "Check timed out after 60.0s" — the execution never becomes SUCCEEDED, so the check's bounded restart-on-FAILED loop never exits. The outcome recorded elsewhere in this suite for a Lambda-invoking Task is a terminal status of FAILED with error "States.Runtime" and an emulator-authored cause naming Invoke as not implemented, with the declared Catch not taken; AWS emits neither that error name nor that cause for a handler exception.
Why: the most common Step Functions resilience pattern — a failing step caught and compensated — is inert. A client sees a FAILED execution where AWS reports SUCCEEDED after compensation, its ErrorEquals matching never fires, $.error is never populated, and the compensating side effect that restores state is silently skipped, leaving the reserved DynamoDB item orphaned.
Fix: the Lambda Invoke data-plane handler in the lambda crate (run the function instead of refusing) and, in the sfn crate, the interpreter's Task failure classification and Catch evaluation — a task failure must carry the AWS error name and the function's structured cause, be matched against each Catch entry's ErrorEquals (including States.ALL), be written to ResultPath and transition to Next so the compensating state runs.
Traces: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure.

## Evidence
Cause 1 — Expected (AWS): StartExecution on the saga state machine ends with DescribeExecution status="SUCCEEDED" (Step1_Reserve writes the reservation item, Step2_Charge's lambda:invoke fails with a Lambda.* error whose cause is the handler's structured error document, the Catch ErrorEquals ["States.ALL"] routes to Compensate with the document at $.error, and Compensate runs), and GetItem(Key={"pk": {"S": "compensation-executed"}}) returns an item.
Actual (emulator): "Check timed out after 60.0s" — no execution ever reaches SUCCEEDED, the Catch target is never entered and the compensation item never appears; the recorded emulator outcome for a Lambda-invoking Task in this suite is status="FAILED" with error="States.Runtime" and an emulator-authored cause stating that Lambda's Invoke operation is not implemented.

## Steps to Reproduce
1. CreateTable; create a Lambda function whose handler raises Exception('ChargeDeclined'); create a second Lambda function that PutItems {"pk": "compensation-executed", "reason": <event JSON>} into the table.
2. Create a role trusted by states.amazonaws.com granting dynamodb:PutItem on the table and lambda:InvokeFunction on both functions.
3. CreateStateMachine (STANDARD): Step1_Reserve = Task arn:aws:states:::dynamodb:putItem -> Step2_Charge = Task arn:aws:states:::lambda:invoke on the failing function with Catch [{"ErrorEquals": ["States.ALL"], "Next": "Compensate", "ResultPath": "$.error"}]; Compensate = Task arn:aws:states:::lambda:invoke on the compensating function with Payload.$ = "$.error".
4. StartExecution with {} and poll DescribeExecution to a terminal status; then GetItem for pk="compensation-executed".
On AWS: status SUCCEEDED and the compensation item exists.
On the emulator: the execution never reaches SUCCEEDED (recorded terminal answer FAILED / States.Runtime with an emulator-authored cause), the Catch branch is never entered, no compensation item appears, and the check is killed at the harness's 60 s cap.
