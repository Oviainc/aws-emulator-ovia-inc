Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.lambda-sdk-putevents-denied-without-permission events__lambda__s3__stepfunctions.s3-upload-lambda-putevents-blocked-without-events-permission
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-putevents-denied-without-permission|run#8|AssertionError|assert denied["ok"] is False, f"expected put_events to be denied without permission, got: {denied}"
Issue-Key: events__lambda__s3__stepfunctions.s3-upload-lambda-putevents-blocked-without-events-permission|run#18|AssertionError|assert not messages, f"rule fired even though Lambda lacked events:PutEvents permission: {messages}"
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# events__lambda__s3__stepfunctions: in-function events:PutEvents succeeds without the permission and the rule still fires

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause behind 2 failing checks.

Cause 1 of 1: IAM is not enforced for `events:PutEvents` called from inside a Lambda — the event is published and delivered even though the execution role grants nothing.
AWS: PutEvents from a handler whose execution role does not allow `events:PutEvents` is refused with HTTP 400 and error code `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: events:PutEvents on resource: <event-bus-arn> because no identity-based policy allows the events:PutEvents action` (fixed text; account, role, function and bus ARN come from the request and the role). Nothing is published, so no rule matches and no target is invoked. Smithy: PutEvents (services/service-com-amazonaws-eventbridge-awsevents) returns per-entry `ErrorCode`/`ErrorMessage` for entry-level failures; an authorization refusal is the request-level service-common AccessDeniedException, not a successful response with FailedEntryCount 0.
Emulator: the unpermitted PutEvents returns success — `{'ok': True, 'EventId': '...'}` with FailedEntryCount 0 — and the downstream rule fires: the check that asserts no delivery finds the event body sitting in the target queue.
Why: a role scoped to forbid publishing has no effect; events leave the function and drive real downstream work (a rule target, a queue message, a state-machine execution) that AWS would never have started. An application validated here will be refused in production, and in the meantime the emulator's downstream state diverges from AWS's.
Fix: the IAM authorization check on in-function AWS SDK calls, in the EventBridge PutEvents handler. The open issue 20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md covers the absent in-runtime SDK and other causes for this service but does not carry these two replay keys.
Traces: the two failing checks in trace_ids, under `### events__lambda__s3__stepfunctions`.

## Evidence
Expected (AWS): HTTP 400 AccessDeniedException `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: events:PutEvents on resource: <bus-arn> because no identity-based policy allows the events:PutEvents action`; no event published and no target invoked.

Actual (emulator):
- lambda-sdk-putevents-denied-without-permission -> AssertionError: expected put_events to be denied without permission, got: {'ok': True, 'EventId': '...'}
- s3-upload-lambda-putevents-blocked-without-events-permission -> AssertionError: rule fired even though Lambda lacked events:PutEvents permission: [{'MessageId': ..., 'Body': '{"version":"0",...,"detail-type":"ObjectProcessed",...}'}] — the event reached the rule's SQS target.

## Steps to Reproduce
1. Create a custom event bus, a rule with an SQS target, and a Lambda whose execution role grants no events actions.
2. Invoke a handler that calls boto3.client('events').put_events(Entries=[...]) against that bus (directly, and via an S3 upload notification).
3. Expected: AccessDeniedException and an empty target queue. Observed: ok=True with an EventId, and the event body in the queue.
