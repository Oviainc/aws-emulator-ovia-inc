Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.lambda-sdk-sqs-missing-receivemessage-permission-negative
Issue-Key: lambda__s3__sqs.lambda-sdk-sqs-missing-receivemessage-permission-negative|run#10|AssertionError|assert denied_result["status"] == "error", f"expected the denied invoke to error, got {denied_result!r}"
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# lambda__s3__sqs: an in-function sqs:ReceiveMessage is served although the execution role has no grant for it (1 cause)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
Cause 1 of 1: no IAM authorization is performed for an SQS read issued from inside a Lambda handler.
AWS: the handler's sqs.receive_message is signed with the execution role's credentials and authorized by IAM. With no sqs:ReceiveMessage Allow for that queue the call is refused — HTTP 403, error code `AccessDenied`, message "User: <principal> is not authorized to perform: sqs:ReceiveMessage on resource: <queue ARN> because no identity-based policy allows the sqs:ReceiveMessage action". Fixed text is everything outside the brackets; <principal> is the caller's assumed-role session ARN and <queue ARN> is the queue named in the request. SQS's Smithy model does not enumerate an authorization error for ReceiveMessage (its errors are OverLimit, QueueDoesNotExist, KmsDisabled, RequestThrottled, UnsupportedOperation, ...), so the requirement here is AWS's documented IAM implicit-deny behavior. The invocation that swallows the refusal must surface it — the invoke reports a FunctionError or the handler returns the error — not a clean success.
Emulator: the read succeeds and the handler returns the fixture body: "expected the denied invoke to error, got {'status': 'ok', 'body': 'receive-denied-fixture'}". The missing grant has no effect at all.
Why: an emulator that serves a read no policy allows makes least-privilege roles untestable; a negative test asserting the denial passes locally and fails in AWS, and data a role must not be able to read is returned.
Fix: the SQS ReceiveMessage handler's authorization check (evaluate the signing principal's identity policies before returning messages), plus the plumbing that makes an in-process Lambda handler's SDK calls carry the execution role as caller identity. The open issue 20261105T094842Z-lambda-s3-sqs-the-python-lambda-runtime-provides.md covers the runtime's missing S3 client for other checks of this suite; it does not carry this replay key and is not about authorization.
Traces: lambda__s3__sqs.lambda-sdk-sqs-missing-receivemessage-permission-negative.

## Evidence
Expected: the handler's ReceiveMessage is refused with HTTP 403 AccessDenied and AWS's implicit-deny message naming the principal, sqs:ReceiveMessage and the queue ARN, so the invoke surfaces an error. Actual: "AssertionError: expected the denied invoke to error, got {'status': 'ok', 'body': 'receive-denied-fixture'}".

## Steps to Reproduce
1. Create an SQS queue with a message on it and a Lambda function whose execution role grants no sqs:ReceiveMessage on that queue.
2. Invoke the function so its handler calls sqs.receive_message.
3. The invocation returns {'status': 'ok', 'body': ...} instead of failing with AccessDenied.
4. Replay lambda__s3__sqs.lambda-sdk-sqs-missing-receivemessage-permission-negative.
