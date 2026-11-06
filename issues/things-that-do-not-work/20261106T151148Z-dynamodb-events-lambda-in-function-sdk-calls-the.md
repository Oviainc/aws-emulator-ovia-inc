Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.cfn-lambda-role-missing-eventbridge-permission-negative dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow dynamodb__events__lambda.lambda-describe-rule-missing-permission-negative dynamodb__events__lambda.lambda-dynamodb-deleteitem-missing-permission-negative dynamodb__events__lambda.lambda-dynamodb-query-missing-permission-negative dynamodb__events__lambda.lambda-putevents-missing-permission-negative dynamodb__events__lambda.lambda-putevents-wrong-event-bus-arn-negative
Issue-Key: dynamodb__events__lambda.cfn-lambda-role-missing-eventbridge-permission-negative|run#11|AssertionError|assert body.get("error_code") == "AccessDeniedException", (
Issue-Key: dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow|run#22|AssertionError|assert payload.get("status") == "error", f"expected the denied invoke to report an error, got: {payload}"
Issue-Key: dynamodb__events__lambda.lambda-describe-rule-missing-permission-negative|run#14|AssertionError|assert denied_result["ok"] is False, denied_result
Issue-Key: dynamodb__events__lambda.lambda-dynamodb-deleteitem-missing-permission-negative|run#12|AssertionError|assert del_body["ok"] is False, (
Issue-Key: dynamodb__events__lambda.lambda-dynamodb-query-missing-permission-negative|run#12|AssertionError|assert query_body["ok"] is False, f"expected denial for an unpermitted Query; got {query_body!r}"
Issue-Key: dynamodb__events__lambda.lambda-putevents-missing-permission-negative|run#16|AssertionError|assert denied_result["ok"] is False, denied_result
Issue-Key: dynamodb__events__lambda.lambda-putevents-wrong-event-bus-arn-negative|_drain_until_marker#22|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# dynamodb__events__lambda: in-function SDK calls the execution role does not allow all succeed, and a wrong-bus PutEvents check hangs (2 causes)

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two root causes behind 7 failing checks.

Cause 1 of 2 (6 checks: cfn-lambda-role-missing-eventbridge-permission-negative, cli-lambda-role-policy-missing-dynamodb-negative-workflow, lambda-describe-rule-missing-permission-negative, lambda-dynamodb-deleteitem-missing-permission-negative, lambda-dynamodb-query-missing-permission-negative, lambda-putevents-missing-permission-negative): IAM is not enforced for AWS SDK calls made from inside a Lambda — every call the execution role has no grant for succeeds.
AWS: an in-function call whose action the execution role does not allow is refused before it touches the service, with HTTP 400 and error code `AccessDeniedException`; the message is the fixed `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: <action> on resource: <resource-arn> because no identity-based policy allows the <action> action`, whose account, role name, function name, action and resource ARN are taken from the request and the role. This holds for `events:PutEvents` on a bus ARN, `events:DescribeRule` on a rule ARN, and `dynamodb:DeleteItem`/`dynamodb:Query` on a table ARN. Smithy: EventBridge PutEvents/DescribeRule (services/service-com-amazonaws-eventbridge-awsevents) and DynamoDB DeleteItem/Query (services/service-com-amazonaws-dynamodb-dynamodb-20120810) model their own client errors; the authorization refusal is the service-common AccessDeniedException.
Emulator: each unpermitted call returns a normal success envelope — `{'ok': True, 'FailedEntryCount': 0, 'Entries': [{'EventId': ...}]}` for PutEvents, `{'ok': True, 'Name': '<rule>', 'State': 'ENABLED'}` for DescribeRule, `{'ok': True}` for DeleteItem, `{'ok': True, 'count': 2}` for Query, `{'sent': True}`, `{'status': 'ok'}`. The event is really published and the item really deleted.
Why: least-privilege has no effect here. A function whose role was deliberately scoped down still reads and mutates data and publishes events; a reviewer who tests against this emulator will ship a role AWS refuses, and negative-path code keyed on AccessDeniedException is dead. For the deliberately-unpermitted writes, state is mutated that AWS would have left untouched.
Fix: the IAM authorization check on in-function AWS SDK calls — the per-operation authorization in the EventBridge and DynamoDB handlers, driven by the invoking function's execution-role policy set.

Cause 2 of 2 (check: lambda-putevents-wrong-event-bus-arn-negative): the wrong-event-bus scenario never reaches a verdict.
AWS: PutEvents naming an `EventBusName` that does not exist is refused with HTTP 400 `ResourceNotFoundException`, message `Event bus <name> does not exist.` (fixed text; the name comes from the request); when the role's policy is scoped to a different bus ARN it is refused with AccessDeniedException instead. Either way the answer arrives in seconds.
Emulator: `Error: Check timed out after 60.0s` — neither refusal is produced.
Why: a typo'd bus name produces a hang rather than a clear refusal, so misrouted events are undiagnosable.
Fix: the EventBridge PutEvents handler's bus-resolution and refusal path.
Traces: the 7 failing checks in trace_ids, under `### dynamodb__events__lambda`. The open issue 20261105T094343Z-dynamodb-events-lambda-the-lambda-python-runtime-provides.md records the absent in-runtime SDK for other checks of this service; it does not carry these replay keys, and these calls now reach the services and are wrongly allowed.

## Evidence
Cause 1 — expected (AWS): HTTP 400 `AccessDeniedException` with `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: <action> on resource: <arn> because no identity-based policy allows the <action> action`.
Actual (emulator):
- lambda-putevents-missing-permission-negative -> AssertionError: {'ok': True, 'FailedEntryCount': 0, 'Entries': [{'EventId': '...'}]}
- cfn-lambda-role-missing-eventbridge-permission-negative -> AssertionError: expected AccessDeniedException from the permission-less caller, got: {'sent': True}
- lambda-describe-rule-missing-permission-negative -> AssertionError: {'ok': True, 'Name': 'sim2real-rule-<id>', 'State': 'ENABLED'}
- lambda-dynamodb-deleteitem-missing-permission-negative -> AssertionError: expected the unpermitted DeleteItem call to be denied, got {'ok': True}
- lambda-dynamodb-query-missing-permission-negative -> AssertionError: expected denial for an unpermitted Query; got {'ok': True, 'count': 2}
- cli-lambda-role-policy-missing-dynamodb-negative-workflow -> AssertionError: expected the denied invoke to report an error, got: {'status': 'ok'}

Cause 2 — expected: HTTP 400 ResourceNotFoundException `Event bus <name> does not exist.` (or AccessDeniedException for a bus-scoped policy). Actual: lambda-putevents-wrong-event-bus-arn-negative -> Error: Check timed out after 60.0s.

## Steps to Reproduce
Cause 1: create a table, a rule and a custom bus. Create a Lambda whose execution role grants ONLY logs actions, and whose handler calls events:PutEvents / events:DescribeRule / dynamodb:DeleteItem / dynamodb:Query. Invoke it. Expected: each call refused with AccessDeniedException. Observed: each call succeeds and the side effect happens.
Cause 2: invoke a handler that calls PutEvents with EventBusName set to a nonexistent bus; expected ResourceNotFoundException within seconds, observed no verdict within 60 s.
