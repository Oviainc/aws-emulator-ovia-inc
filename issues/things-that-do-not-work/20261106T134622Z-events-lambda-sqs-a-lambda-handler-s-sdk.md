Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.lambda-putevents-missing-permission-denied events__lambda__sqs.lambda-role-without-events-putevents-permission-denied events__lambda__sqs.lambda-role-without-sqs-sendmessage-permission-denied events__lambda__sqs.lambda-sdk-call-sqs-missing-permission-denied events__lambda__sqs.lambda-sdk-events-describerule-missing-permission-denied events__lambda__sqs.lambda-sdk-putevents-missing-permission-denied events__lambda__sqs.lambda-sdk-sqs-receivemessage-missing-permission-denied events__lambda__sqs.cli-delete-event-source-mapping-stops-polling
Issue-Key: events__lambda__sqs.lambda-putevents-missing-permission-denied|run#18|AssertionError|assert payload["ok"] is False, f"expected put_events to be denied, got: {payload}"
Issue-Key: events__lambda__sqs.lambda-role-without-events-putevents-permission-denied|_poll_until#26|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.lambda-role-without-sqs-sendmessage-permission-denied|run#7|AssertionError|assert denied["payload"]["ok"] is False, (
Issue-Key: events__lambda__sqs.lambda-sdk-call-sqs-missing-permission-denied|run#9|AssertionError|assert payload["ok"] is False, f"expected the SDK call to fail without permission, got: {payload}"
Issue-Key: events__lambda__sqs.lambda-sdk-events-describerule-missing-permission-denied|run#11|AssertionError|assert denied_body["ok"] is False, (
Issue-Key: events__lambda__sqs.lambda-sdk-putevents-missing-permission-denied|run#22|AssertionError|assert denied_body["ok"] is False, f"expected the denied call to fail, got: {denied_body}"
Issue-Key: events__lambda__sqs.lambda-sdk-sqs-receivemessage-missing-permission-denied|run#9|AssertionError|assert denied_body["ok"] is False, (
Issue-Key: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling|_drain_queue_for#23|Timeout|time.sleep(interval_s)
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# events__lambda__sqs: a Lambda handler's SDK calls are never authorized against its execution role, and deleting an event source mapping never stops polling (2 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two net-new root causes. The open issues on this suite (20261029T182228Z-events-lambda-sqs-6-of-the-7-causes-remain-open.md, 20261101T210635Z-events-lambda-sqs-6-net-new-causes-eventbridge.md, 20261105T094343Z-events-lambda-sqs-the-lambda-python-runtime-provides.md) cover EventBridge emission, durable-execution status events and the runtime's missing SDK clients; none carries these replay keys and none says that authorization is skipped.

Cause 1 of 2: no IAM authorization is performed for SDK calls made from inside a function — seven checks, every one of them a call that AWS refuses and the emulator serves.
AWS: a function's SDK calls are signed with its execution-role credentials and authorized by IAM. With no matching Allow (or after the grant is revoked), the call is refused, not served: EventBridge PutEvents/DescribeRule answer the IAM implicit-deny (HTTP 400, code `AccessDeniedException`), SQS SendMessage/ReceiveMessage answer HTTP 403 code `AccessDenied`. The message is AWS's implicit-deny template "User: <principal> is not authorized to perform: <action> on resource: <resource ARN> because no identity-based policy allows the <action> action" — the fixed text is everything outside the brackets; <principal> is the caller's assumed-role session ARN, <action> is the IAM action for the operation, and <resource ARN> is the rule/queue named in the request. These authorization errors are not enumerated in the SQS or EventBridge Smithy models, so the rule here is AWS's documented IAM behavior, not a modeled error.
Emulator: every one of these calls succeeds. events:PutEvents returns {'ok': True} / {'ok': True, 'failed': 0}; sqs:SendMessage returns {'ok': True, 'messageId': '<uuid>'}; events:DescribeRule returns the rule ({'ok': True, 'state': 'ENABLED', 'arn': '<rule ARN>'}); sqs:ReceiveMessage returns ok=True. Revoking the grant afterwards changes nothing — one check polls for 30 s and the call keeps succeeding.
Why: least-privilege design is unverifiable and silently wrong — a role with no grant at all behaves exactly like a fully privileged one, so a negative test asserting denial passes on the emulator and fails on AWS, and a real deployment hits an AccessDenied the emulator never showed.
Fix: the request-authorization layer shared by the EventBridge and SQS operation handlers (evaluate the signing principal's identity policies before serving), plus the plumbing that makes an in-process Lambda handler's calls carry the execution role as caller identity.
Traces: events__lambda__sqs.lambda-putevents-missing-permission-denied, .lambda-role-without-events-putevents-permission-denied, .lambda-role-without-sqs-sendmessage-permission-denied, .lambda-sdk-call-sqs-missing-permission-denied, .lambda-sdk-events-describerule-missing-permission-denied, .lambda-sdk-putevents-missing-permission-denied, .lambda-sdk-sqs-receivemessage-missing-permission-denied.

Cause 2 of 2: the delete-event-source-mapping scenario never reaches a verdict.
AWS: DeleteEventSourceMapping puts the mapping into Deleting and then removes it; polling stops, so messages sent to the source queue afterwards stay on the queue and the function is not invoked again. Smithy: DeleteEventSourceMapping returns EventSourceMappingConfiguration and errors ResourceNotFoundException / ResourceInUseException.
Emulator: "Error: Check timed out after 60.0s" — the check is killed before it can assert anything, so neither the pre-delete consumption nor the post-delete quiescence is observable.
Why: a client cannot confirm that tearing a mapping down actually stops consumption; a mapping that keeps draining a queue after deletion destroys messages.
Fix: the SQS event-source-mapping runtime (poller lifecycle on delete) and DeleteEventSourceMapping's state transition.
Traces: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling.

## Evidence
Cause 1 — expected: each call is refused with the service's access-denied error and AWS's implicit-deny message naming the principal, the action and the resource. Actual, from the replay: "expected put_events to be denied, got: {'ok': True}"; "events:PutEvents to be denied once the grant is revoked not met within 30.0s. Last result: (..., {'status_code': 200, 'payload': {'ok': True}})"; "sqs:SendMessage must be denied without an IAM grant, got: {'status_code': 200, 'payload': {'ok': True, 'messageId': '...'}}"; "expected the SDK call to fail without permission, got: {'ok': True, 'messageId': '...'}"; "expected the DescribeRule call to be denied, but the role without events:DescribeRule succeeded: {'ok': True, 'state': 'ENABLED', 'arn': 'arn:aws:events:us-west-2:...:rule/.../...'}"; "expected the denied call to fail, got: {'ok': True, 'failed': 0}"; "a role lacking sqs:ReceiveMessage must have the SDK call fail inside the function; got ok=True". (ids/message ids are per-run values.)

Cause 2 — expected: after DeleteEventSourceMapping, messages sent to the source queue are not consumed. Actual: "Error: Check timed out after 60.0s" — no verdict.

## Steps to Reproduce
1. Create an EventBridge bus/rule, an SQS queue, and a Lambda function whose handler calls events:PutEvents, events:DescribeRule, sqs:SendMessage and sqs:ReceiveMessage.
2. Give the execution role no grant for the action under test (or revoke it after a successful control call) and invoke the function. Every call still succeeds with ok=True instead of being refused.
3. For cause 2, create an SQS event source mapping, confirm it consumes, delete it with DeleteEventSourceMapping, then send more messages. The check is killed at 60 s with no verdict.
4. Replay the eight check ids in trace_ids.
