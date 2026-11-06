Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-missing-permission-negative lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative
Issue-Key: lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-missing-permission-negative|run#8|AssertionError|assert denied_result["ok"] is False, f"handler unexpectedly succeeded: {denied_result}"
Issue-Key: lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative|_drain_json_messages#30|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# lambda__pipes__sqs: an in-function sqs:SendMessage is served without an IAM grant, and a disabled event source mapping scenario never settles (2 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Two net-new root causes. The open issues on this suite (20261104T213346Z-lambda-pipes-sqs-5-net-new-causes-an.md, 20261102T094631Z-lambda-pipes-sqs-2-net-new-causes-a.md, 20261105T094842Z-lambda-pipes-sqs-the-python-lambda-runtime-provides.md) cover pipe wiring, pipe delivery and the runtime's missing Pipes client; none carries these replay keys.

Cause 1 of 2: the execution role's identity policy is not evaluated for an SQS call made from inside the function.
AWS: the handler's sqs.send_message is signed with the execution role and authorized by IAM. With no sqs:SendMessage Allow for the target queue the call is refused — HTTP 403, code `AccessDenied`, message "User: <principal> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no identity-based policy allows the sqs:SendMessage action" (fixed text outside the brackets; <principal> is the assumed-role session ARN and <queue ARN> the queue in the request). SQS's Smithy model does not enumerate an authorization error for SendMessage, so this is AWS's documented IAM behavior.
Emulator: the handler succeeds — "handler unexpectedly succeeded: {'ok': True, 'message_id': '<uuid>'}" — so the absent grant has no effect and a message is actually enqueued.
Why: a negative test that proves a function cannot write to a queue passes here and fails on AWS; worse, the emulator reports a message on a queue that AWS would never have accepted, so downstream assertions are built on a write that cannot happen in production.
Fix: the SQS SendMessage handler's authorization check (consult the signing principal's identity policies before enqueuing) and the plumbing that gives an in-process handler's calls the execution-role identity.
Traces: lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-missing-permission-negative.

Cause 2 of 2: the disabled-mapping scenario never reaches a verdict.
AWS: an event source mapping created with Enabled=false (or updated to it) reports State `Disabled` and does not poll, so a message sent to the source queue stays there and the function is not invoked; enabling it resumes consumption. Smithy: CreateEventSourceMapping/UpdateEventSourceMapping carry `Enabled` and return EventSourceMappingConfiguration{State,...}.
Emulator: "Error: Check timed out after 60.0s" — the check is killed before asserting, so neither the quiescence while disabled nor the resumption is observable.
Why: a client cannot verify that disabling a mapping stops consumption — the control that is used to pause a consumer during maintenance is unverifiable, and a hang looks like an outage.
Fix: the SQS event-source-mapping runtime's enable/disable handling (poller lifecycle and the State the configuration reports).
Traces: lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative.

## Evidence
Cause 1 — expected: the handler's SendMessage is refused (HTTP 403, AccessDenied, implicit-deny message naming the principal, sqs:SendMessage and the queue ARN). Actual: "AssertionError: handler unexpectedly succeeded: {'ok': True, 'message_id': '01242c3c-...'}" (the message id is a per-run value).

Cause 2 — expected: with the mapping disabled, a message sent to the source queue is not consumed and the function is not invoked. Actual: "Error: Check timed out after 60.0s" — no verdict.

## Steps to Reproduce
1. Create an SQS queue and a Lambda function whose execution role has no sqs:SendMessage grant for it; invoke the function so its handler sends a message. It succeeds and returns a message id.
2. Create an SQS event source mapping with Enabled=false, send a message to the source queue and wait for the no-invocation window. The check is killed at 60 s with no verdict.
3. Replay lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-missing-permission-negative and lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative.
