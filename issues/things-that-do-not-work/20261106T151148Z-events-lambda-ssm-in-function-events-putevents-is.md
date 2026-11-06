Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__ssm.lambda-without-events-permission-denied events__lambda__ssm.lambda-events-putevents-wrong-bus-arn-denied
Issue-Key: events__lambda__ssm.lambda-without-events-permission-denied|run#8|AssertionError|assert payload["denied"] is True, f"expected PutEvents to be denied, got {payload}"
Issue-Key: events__lambda__ssm.lambda-events-putevents-wrong-bus-arn-denied|run#19|AssertionError|assert denied_result["denied"] is True, (
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# events__lambda__ssm: in-function events:PutEvents is accepted with no permission and for a bus the policy does not cover

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause behind 2 failing checks.

Cause 1 of 1: `events:PutEvents` called from inside a Lambda is never authorized — neither the absence of a grant nor a grant scoped to a different event bus refuses it.
AWS: PutEvents from a handler is refused with HTTP 400 and error code `AccessDeniedException` when the execution role does not allow `events:PutEvents`, and equally when the role's policy names a different event-bus ARN than the bus in the request. The message is the fixed `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: events:PutEvents on resource: <event-bus-arn> because no identity-based policy allows the events:PutEvents action`, with the account, role name, function name and bus ARN taken from the request and the role. Smithy: PutEvents (services/service-com-amazonaws-eventbridge-awsevents) carries per-entry ErrorCode/ErrorMessage for entry failures; an authorization refusal is a request-level AccessDeniedException, not a 200 with FailedEntryCount 0.
Emulator: both the no-grant case and the wrong-bus-ARN case return `{'ok': True, 'denied': False, 'mode': 'accepted', 'code': None}` — accepted, with no error code at all.
Why: resource-scoped EventBridge policies are inert, so a function can publish to any bus in the account regardless of its role; an application validated here is refused on AWS, and the events it publishes in the meantime drive downstream work AWS would not have started.
Fix: the IAM authorization check on in-function AWS SDK calls in the EventBridge PutEvents handler, including resource-ARN matching for the target bus. The open issue 20261105T081225Z-events-lambda-ssm-the-lambda-python-runtime-has.md records the absent in-runtime SDK for the SSM half of this service; it does not carry these two replay keys.
Traces: the two failing checks in trace_ids, under `### events__lambda__ssm`.

## Evidence
Expected (AWS): HTTP 400 AccessDeniedException naming `events:PutEvents` and the event-bus ARN, in both the no-grant and the wrong-bus-ARN cases.

Actual (emulator):
- events__lambda__ssm.lambda-without-events-permission-denied -> AssertionError: expected PutEvents to be denied, got {'ok': True, 'denied': False, 'mode': 'accepted', 'code': None}
- events__lambda__ssm.lambda-events-putevents-wrong-bus-arn-denied -> AssertionError: expected AccessDenied for wrong event bus, got {'ok': True, 'denied': False, 'mode': 'accepted', 'code': None}

## Steps to Reproduce
1. Create two custom event buses, A and B.
2. Create a Lambda whose role (a) grants nothing, then (b) grants events:PutEvents only on bus A's ARN.
3. Invoke a handler that calls put_events against bus B in each case. Expected: AccessDeniedException both times. Observed: accepted, ok=True, code None.
