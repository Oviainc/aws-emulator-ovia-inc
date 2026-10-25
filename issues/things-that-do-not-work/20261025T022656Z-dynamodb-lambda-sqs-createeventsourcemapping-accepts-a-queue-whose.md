Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq
Issue-Key: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix|run#12|AssertionError|raise AssertionError(
Issue-Key: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq|_poll_item#39|Timeout|time.sleep(2)
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# dynamodb__lambda__sqs: CreateEventSourceMapping accepts a queue whose visibility timeout is below the function timeout, and the DLQ/permission-revocation flow never completes (2 causes)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 2: an SQS event source mapping is created even though the queue's VisibilityTimeout is smaller than the function's Timeout.
AWS: Lambda refuses this combination at CreateEventSourceMapping — the queue's visibility timeout must be at least the function timeout, and the request is rejected with InvalidParameterValueException (Smithy (lambda) CreateEventSourceMapping models InvalidParameterValueException, httpError 400). Recorded behavior only for the exact message.
Emulator: the call succeeds and returns a mapping for a queue with VisibilityTimeout=2 s against a function with Timeout=5 s.
Why: the client is allowed to build a mapping that re-delivers in-flight messages mid-invocation; the misconfiguration surfaces later as duplicate processing instead of at the call that created it, which is exactly what AWS's validation prevents.
Fix: the CreateEventSourceMapping handler's validation in the lambda service — compare the resolved queue's VisibilityTimeout with the function's Timeout before accepting.
Traces: dynamodb__lambda__sqs.sqs-dlq-redrive-reprocesses-after-function-fix.

Cause 2 of 2: the revoked-downstream-permission flow never reaches a verdict and is killed at the 60 s cap.
AWS: when an SQS-triggered function's role loses the downstream permission, the invocation fails with the authorization error naming the denied action and principal, the message is retried and ultimately lands in the queue's configured dead-letter queue. Recorded behavior only for the message text.
Emulator: the check makes no progress — 'Error: Check timed out after 60.0s'. No invocation, no authorization failure and no DLQ delivery is observable.
Why: a client cannot verify that a permission revocation is enforced, nor that failures are captured by the DLQ it configured; the whole failure-handling contract of an SQS-triggered function is invisible.
Fix: the SQS event-source-mapping poller/delivery path in the lambda service (consume, invoke, retry, redrive to DLQ) and the IAM authorization of the function's downstream call.
Traces: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq.

## Evidence
Cause 1 — Expected: CreateEventSourceMapping is refused InvalidParameterValueException (400) when queue VisibilityTimeout (2 s) < function Timeout (5 s). Actual: AssertionError: an ESM on a queue whose VisibilityTimeout (2s) is below the function Timeout (5s) must be refused — the call succeeded.
Cause 2 — Expected: the failing invocation is denied, retried and the message lands in the DLQ. Actual: Error: Check timed out after 60.0s.

## Steps to Reproduce
Cause 1: create a queue with VisibilityTimeout=2, a function with Timeout=5, then CreateEventSourceMapping(EventSourceArn=queue, FunctionName=fn).
Cause 2: wire an SQS queue with a redrive policy to a function, revoke the function role's downstream permission, send a message and poll the DLQ.
