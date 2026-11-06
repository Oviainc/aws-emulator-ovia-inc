Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied
Issue-Key: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied|run#21|AssertionError|assert denied_payload["ok"] is False, (
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# events__lambda__sns: a Lambda handler's events:PutEvents succeeds under an execution role that grants it nothing — no AccessDeniedException, and the event reaches the bus

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
This issue carries 1 net-new root cause. It must be closed before submitting. Related but NOT covering: the open issues 20261105T131255Z-events-lambda-sns-a-lambda-handler-cannot-call.md and 20261105T175821Z-events-lambda-sns-a-python-lambda-handler-that.md describe the earlier symptom for this same check (the handler could not import boto3 at all, Invoke answered FunctionError=Unhandled / Runtime.ImportModuleError). The handler now runs and returns its own payload, so the suite reports this failure as unowned; what is left is the authorization gap below.

Cause 1 of 1: EventBridge PutEvents called from inside a Lambda function is never authorized against the function's execution role.
AWS: the managed python runtimes inject the execution role's temporary credentials, so a client built in the handler is signed as `arn:aws:sts::<account-id>:assumed-role/<role-name>/<function-name>` and PutEvents is authorized against that role's identity policies. The model carries no IAM-denial error for this operation (Smithy: PutEvents(Entries required, @length 1..10) -> {FailedEntryCount default 0, Entries}; the only modeled error is InternalException, httpError 500), so the denial is the service-common one — recorded behavior: the call is REFUSED with error code `AccessDeniedException` and the message `User: <assumed-role-principal> is not authorized to perform: events:PutEvents on resource: <event-bus-arn> because no identity-based policy allows the events:PutEvents action`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`; variable parts, from the request and state: the assumed-role principal ARN (account id, the role name of the function's Role, the function name), the action `events:PutEvents` and the target event bus ARN. A refused PutEvents puts nothing on the bus, so no rule matches and no target is delivered to. With the grant present the same call returns FailedEntryCount=0 and the entry reaches the rule's target.
Emulator: the call succeeds for a role carrying only AWSLambdaBasicExecutionRole — the handler payload is `{'ok': True, 'failed_count': 0}`, i.e. a 200 PutEvents response with FailedEntryCount 0, and no AccessDeniedException is produced anywhere. The check fails on `assert denied_payload["ok"] is False` with `AssertionError: expected the role lacking events:PutEvents to fail the call, got ok=True: {'ok': True, 'failed_count': 0}`; the denial's second half (no event on the bus) is therefore unverifiable as well.
Why: a client relying on AWS's behavior loses the permission boundary around event publication. An under-permissioned function publishes happily in the emulator and is refused in AWS, so least-privilege roles cannot be validated, and any negative test asserting "the denied producer puts nothing on the bus" passes vacuously (or, worse, the denied event really does arrive and fan out to downstream targets).
Fix: the authorization step of the PutEvents handler in the eventbridge crate, plus the identity the Lambda execution engine's guest SDK presents when it dispatches into that front door — the guest call must carry the function's execution-role principal and PutEvents must evaluate it against the role's identity policies (and the resource policy of the target bus) before accepting entries.
Traces: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied (the same cause under the lambda__sns__sqs heading is filed separately in this sweep for SQS/SNS calls).

## Evidence
events__lambda__sns.lambda-role-missing-events-permission-putevents-denied
Expected (AWS): the handler using a role with no events:PutEvents grant catches an AccessDeniedException — payload {'ok': False, 'error_code': 'AccessDeniedException'} — and no event with the denied marker ever reaches the rule's queue; the sibling function whose role grants events:PutEvents returns {'ok': True, 'failed_count': 0} and its event does reach the queue.
Actual (emulator):
  File "check_events__lambda__sns_lambda_role_missing_events_permission_putevents_denied.py", line 87, in run
    assert denied_payload["ok"] is False
  AssertionError: expected the role lacking events:PutEvents to fail the call, got ok=True: {'ok': True, 'failed_count': 0}

## Steps to Reproduce
Replay events__lambda__sns.lambda-role-missing-events-permission-putevents-denied.
Minimal repro: create an IAM role trusted by lambda.amazonaws.com with only arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole attached (no events:* grant); CreateFunction (Runtime=python3.12, Handler=index.handler) whose handler calls boto3.client('events').put_events(Entries=[{Source, DetailType, Detail}]) inside try/except and returns {'ok': True, 'failed_count': resp['FailedEntryCount']} or the caught error code; Invoke (RequestResponse). AWS answers with the handler's caught AccessDeniedException and nothing reaches an EventBridge rule matching that Source; the emulator answers {'ok': True, 'failed_count': 0}.
