Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.sqs-fifo-lambda-message-group-order-preserved
Issue-Key: events__lambda__sqs.sqs-fifo-lambda-message-group-order-preserved|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# events__lambda__sqs: a FIFO-queue event source mapping never delivers, so per-message-group ordering is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an event source mapping on a FIFO queue never invokes the function, so the ordering guarantee the check must observe is never produced; the check is killed at the harness 60 s cap.

AWS: an ESM on a FIFO queue delivers each message group's messages to the function in order and does not start the next batch for a group until the previous one is handled, so a consumer that records what it received reproduces the send order within each MessageGroupId. Recorded behavior only for the delivery delay.

Emulator: nothing is delivered — the check polls for the consumer's record of the ordered messages and reports 'Error: Check timed out after 60.0s'.

Why: FIFO ordering is the only reason to choose a FIFO queue; a client gets no invocations at all, so it cannot tell whether ordering holds, and a waiter on the consumer's output never resolves.

Fix: the SQS event-source-mapping poller in the lambda service, specifically FIFO handling (message-group serialisation) on top of the basic consume-and-invoke path. Related open issues 20261025T002357Z-events-lambda-sqs-eventbridge-lambda-targets-and-sqs-event-source-mappings.md and 20261024T043150Z-lambda-sqs-event-source-mapping-never-consumes-messages.md cover neighbouring causes but do not list this check.

Traces: events__lambda__sqs.sqs-fifo-lambda-message-group-order-preserved.

## Evidence
Expected: messages sent with one MessageGroupId are delivered to the function in send order and the consumer's record reproduces that order.
Actual: Error: Check timed out after 60.0s — no invocation ever happened.

## Steps to Reproduce
1. Create a FIFO queue and a function that records each received message.
2. CreateEventSourceMapping(EventSourceArn=<fifo queue>, FunctionName=fn).
3. SendMessage several messages with the same MessageGroupId and distinct sequence markers.
4. Poll the consumer's record and observe nothing arrives.
