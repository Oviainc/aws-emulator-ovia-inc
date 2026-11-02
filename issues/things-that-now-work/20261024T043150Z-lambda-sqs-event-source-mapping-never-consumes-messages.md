Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sqs-esm-visibility-timeout-extended-during-processing
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-visibility-timeout-extended-during-processing|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# Lambda SQS event source mapping never consumes messages, so no invocation and no visibility-timeout extension is observable

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an enabled SQS event source mapping never polls its queue.
AWS: once `CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=..., Enabled=true)` reaches `State: Enabled`, Lambda's poller receives messages from the queue, invokes the function with the SQS `Records` envelope, extends the in-flight messages' visibility for the duration of processing, and deletes them on a successful invocation.
Emulator: nothing is ever received or invoked, so neither the invocation nor the visibility extension ever becomes observable; the check makes no progress and is killed by the harness with `Check timed out after 60.0s`.
Why: every queue-driven consumer deployed through an event source mapping is inert. A client's integration test hangs rather than failing, and the at-least-once/visibility semantics it depends on cannot be exercised.
Fix: the event-source-mapping poller component of the lambda crate (the runtime that drives SQS Receive -> Invoke -> Delete and the visibility extension), plus its interaction with the sqs state. Related open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the Invoke stub itself and carries the replay keys for two other checks; it does not name the ESM poller or this check.
Traces: lambda__sfn__sqs.sqs-esm-visibility-timeout-extended-during-processing.

## Evidence
Expected: the function is invoked with the queue's messages and the messages' visibility timeout is extended while processing. Actual: no invocation and no visibility change ever appears; the harness reports 'Check timed out after 60.0s'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Create an SQS queue and a Lambda function, then CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, Enabled=true) and wait for State='Enabled'.
3. SendMessage to the queue and poll for the function's side effect and for the message's visibility timeout — neither ever changes.

## Resolution (partial: the poller and the visibility hold)

The poller now exists and consumes: an enabled mapping on a queue receives the queue's
messages through the queue's own delivery bookkeeping, applies the queue's RedrivePolicy and
the mapping's FilterCriteria, hands the admitted messages to its function as the SQS
`Records` envelope, deletes exactly what the filter rejected or an invocation accepted, and
meters the pass. The visibility leg named in this report is what was added last: a polled
batch now stays invisible to every other consumer for the whole span the attempt it was
handed to may take — the longer of the queue's `VisibilityTimeout` and the invoked
function's `Timeout` — so no message is delivered again while the first attempt at it may
still be running.

What is NOT covered here: RUNNING the deployment package. `Invoke` still answers HTTP 501
for `RequestResponse`, because this emulator has no in-process runtime for a zip or image
package, so a function's own side effect never becomes observable. That is the separate open
issue this report names — `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`,
with `20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md` —
and until one of them lands, the trace above still waits for that side effect.
