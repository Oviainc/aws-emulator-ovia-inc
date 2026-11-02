Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-events-trust-rejected-at-delivery-failed-to-assume-role events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-start-execution-denied-dead-letters-and-counts-failed-invocations
Issue-Key: events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-events-trust-rejected-at-delivery-failed-to-assume-role|_differential_rounds#29|AssertionError|assert code == "FAILED_TO_ASSUME_ROLE", (
Issue-Key: events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-start-execution-denied-dead-letters-and-counts-failed-invocations|_differential_rounds#28|AssertionError|assert code == "FAILED_TO_ASSUME_ROLE", (
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# EventBridge dead-letter ERROR_CODE misclassifies target-delivery failures: trust failure reported NO_PERMISSIONS and authorization failure reported ACCESS_DENIED (2 causes)

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
EventBridge does dead-letter the undeliverable event (so the DLQ path itself works), but it stamps the wrong ERROR_CODE message attribute on it. The two distinct failure classes AWS separates are collapsed/shifted by one, so a consumer that routes on ERROR_CODE mis-triages both. Two causes, both in the target-delivery failure classifier.

Cause 1 of 2: a target role whose TRUST policy does not allow events.amazonaws.com is dead-lettered as NO_PERMISSIONS instead of FAILED_TO_ASSUME_ROLE.
AWS: recorded behavior only — the Smithy model describes DeadLetterConfig/RoleArn on PutTargets.Target but does not enumerate the DLQ message attributes. The recording shows EventBridge writing the event to the target's dead-letter queue with message attributes RULE_ARN, TARGET_ARN, ERROR_CODE and ERROR_MESSAGE, and ERROR_CODE = 'FAILED_TO_ASSUME_ROLE' when the role cannot be assumed by events.amazonaws.com at all.
Emulator: ERROR_CODE = 'NO_PERMISSIONS'. Its ERROR_MESSAGE diagnoses the right thing (example: "Event bus failed to assume role arn:aws:iam::...:role/...: its trust policy does not let events.amazonaws.com perform sts:AssumeRole."), so only the classification is wrong, not the detection.
Why: a client (or an operator runbook) that distinguishes "fix the trust policy" from "fix the permission policy" by ERROR_CODE is sent to the wrong remediation; alarms keyed on FAILED_TO_ASSUME_ROLE never fire.
Fix: the EventBridge target-delivery path's failure-to-ERROR_CODE mapping, where the assume-role step's failure is classified before the authorization step's.
Traces: events__iam__sfn__sqs.xsvc-iam-3-role-without-events-trust-rejected-at-delivery-failed-to-assume-role

Cause 2 of 2: a target role that CAN be assumed but lacks states:StartExecution is dead-lettered as ACCESS_DENIED instead of NO_PERMISSIONS.
AWS: recorded behavior only — recording shows ERROR_CODE = 'NO_PERMISSIONS' for an assumable role whose permission policy does not allow the target action.
Emulator: ERROR_CODE = 'ACCESS_DENIED', a value the recorded EventBridge DLQ taxonomy does not use. ERROR_MESSAGE example: "Role arn:aws:iam::...:role/... is not authorized to perform states:StartExecution on arn:aws:states:...:stateMachine:...".
Why: same mis-triage in the other direction, and ACCESS_DENIED is not a value any EventBridge DLQ consumer is written to expect, so the branch handling permission failures is dead code against this emulator.
Fix: same classifier — the authorization-failure branch must emit NO_PERMISSIONS.
Traces: events__iam__sfn__sqs.xsvc-iam-3-role-without-start-execution-denied-dead-letters-and-counts-failed-invocations

Related but not covering: 20261022T194351Z-events-firehose-s3-...md reports that failed target invocations never reach the DLQ at all; here they DO reach it, with wrong codes.

## Evidence
Cause 1 — expected ERROR_CODE 'FAILED_TO_ASSUME_ROLE', actual: AssertionError: DLQ ERROR_CODE='NO_PERMISSIONS' for the broken rule, expected 'FAILED_TO_ASSUME_ROLE'. ERROR_MESSAGE='Event bus failed to assume role arn:aws:iam::000000000000:role/sim2real-xsvc-iam3-notrust-...: its trust policy does not let events.amazonaws.com perform sts:AssumeRole.'

Cause 2 — expected ERROR_CODE 'NO_PERMISSIONS', actual: AssertionError: DLQ ERROR_CODE='ACCESS_DENIED' for the broken rule, expected 'NO_PERMISSIONS'. ERROR_MESSAGE='Role arn:aws:iam::000000000000:role/sim2real-xsvc-iam3-badr-... is not authorized to perform states:StartExecution on arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-xsvc-iam3-bad-...'

## Steps to Reproduce
1. Create an SQS queue to serve as a dead-letter queue and a Step Functions state machine.
2. Create an EventBridge rule with an event pattern and a state-machine target whose Target carries DeadLetterConfig pointing at that queue.
3. Cause 1: give the target RoleArn an inline policy allowing states:StartExecution but a trust policy that does NOT allow events.amazonaws.com to assume it. PutEvents a matching event, receive the DLQ message, read the ERROR_CODE message attribute — expect FAILED_TO_ASSUME_ROLE, emulator returns NO_PERMISSIONS.
4. Cause 2: use a role that events.amazonaws.com CAN assume but whose policy omits states:StartExecution. PutEvents a matching event, read ERROR_CODE — expect NO_PERMISSIONS, emulator returns ACCESS_DENIED.
