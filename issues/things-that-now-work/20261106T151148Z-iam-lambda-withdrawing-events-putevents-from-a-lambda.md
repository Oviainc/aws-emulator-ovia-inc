Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.lambda-cannot-put-events-without-permission
Issue-Key: iam__lambda.lambda-cannot-put-events-without-permission|_poll_until#21|AssertionError|raise AssertionError(
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# iam__lambda: withdrawing events:PutEvents from a Lambda execution role never takes effect

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause behind 1 failing check.

Cause 1 of 1: after the `events:PutEvents` grant is removed from a function's execution role, the in-function call keeps succeeding — the authorization decision never reflects the policy change.
AWS: once the identity policy granting `events:PutEvents` is deleted (DeleteRolePolicy / DetachRolePolicy) the next in-function PutEvents is refused with HTTP 400 and error code `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: events:PutEvents on resource: <event-bus-arn> because no identity-based policy allows the events:PutEvents action` (fixed text; account, role, function and bus ARN come from the request and the role). The change is effective within seconds — AWS does not cache an old allow indefinitely. Model: IAM DeleteRolePolicy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508) and EventBridge PutEvents (services/service-com-amazonaws-eventbridge-awsevents).
Emulator: for the whole 30 s the check polls, every invocation returns `{'ok': True, 'code': None, 'message': None}` — the withdrawn permission is still honoured as an allow, and the failure carries no code or message when it is finally graded.
Why: revoking a permission is the primary incident response to a compromised or over-privileged function. Here revocation is a no-op, so the function keeps publishing events after an operator believes it has been locked out — a security-relevant divergence, and one that also makes IAM change testing worthless.
Fix: the IAM authorization check on in-function AWS SDK calls — it must evaluate the role's current policy set on every call (no stale allow), and refuse with AccessDeniedException. The open issue 20261105T085348Z-iam-lambda-2-net-new-root-causes-invokefunction.md covers InvokeFunction authorization and the absent in-runtime SDK for this service; it does not carry this replay key.
Traces: iam__lambda.lambda-cannot-put-events-without-permission.

## Evidence
Expected (AWS): after the policy is deleted, the next in-function PutEvents is refused with AccessDeniedException naming events:PutEvents and the bus ARN.

Actual (emulator): AssertionError: permission withdrawal to take effect not met within 30.0s. Last result: {'ok': True, 'code': None, 'message': None}.

## Steps to Reproduce
1. Create a Lambda whose role has an inline policy allowing events:PutEvents; invoke a handler that calls put_events and confirm it succeeds.
2. DeleteRolePolicy to remove that grant.
3. Re-invoke repeatedly. Expected: AccessDeniedException within seconds. Observed: ok=True for at least 30 s.
