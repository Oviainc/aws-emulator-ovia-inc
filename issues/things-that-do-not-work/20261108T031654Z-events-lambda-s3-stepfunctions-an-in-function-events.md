Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative|run#23|AssertionError|assert payload1["ok"] is False, f"expected the emitter to report failure, got {payload1!r}"
Issue-Commit: f3f9216a884ddb496f260c89a030775b09cc5916

# events__lambda__s3__stepfunctions: an in-function events:PutEvents succeeds although the execution role does not grant it

> _Note: this issue was identified against the code base at commit `f3f9216a884ddb496f260c89a030775b09cc5916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: an AWS call made from inside a Lambda handler is not authorized against the function's execution role.
AWS: a caller whose identity policy does not allow events:PutEvents is refused AccessDeniedException with HTTP 400 and the message `User: <caller role-session ARN> is not authorized to perform: events:PutEvents on resource: <event bus ARN> because no identity-based policy allows the events:PutEvents action` — the fixed text is `User: … is not authorized to perform: events:PutEvents on resource: … because no identity-based policy allows the events:PutEvents action`, with the principal ARN taken from the invocation's identity and the bus ARN from the request. The handler therefore reports failure, and no event is delivered to the downstream target until the permission is granted.
Emulator: the handler's put_events call succeeds — it reports `{'ok': True}` — even though the CloudFormation-deployed execution role carries no events:PutEvents statement, so the emitter publishes the event and the 'ungranted' half of the scenario is indistinguishable from the granted one.
Why: least privilege is unenforced for in-function calls. A client that scopes an execution role gets no effect from it, an over-permissive deployment cannot be detected, and a negative test that should prove the boundary passes while the event is actually delivered.
Fix: the authorization layer the in-process Lambda runtime's AWS client goes through — the in-function call must be evaluated against the execution role's identity policy and refused AccessDeniedException with AWS's wording before the operation runs.
Traces: events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative. Related open issue that does not carry this check's replay key: 20261106T151148Z-events-lambda-s3-stepfunctions-in-function-events-putevents.md.

## Evidence
Expected: the emitter reports failure (AccessDeniedException from events:PutEvents) while its role lacks the permission. Actual: `AssertionError: expected the emitter to report failure, got {'ok': True}`.

## Steps to Reproduce
Deploy a stack with a Lambda whose execution role has no events:PutEvents statement and a handler that calls events.put_events on a custom bus, catching ClientError and returning {'ok': False, …} on refusal; Invoke it. Observe {'ok': True}. Re-replay events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative.
