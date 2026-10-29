Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__sqs.subscription-filter-non-matching-pattern-not-delivered
Issue-Key: lambda__logs__sqs.subscription-filter-non-matching-pattern-not-delivered|_drain#17|Timeout|resp = sqs.receive_message(
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# lambda__logs__sqs: the non-matching-subscription-filter check hangs to the 60 s cap because its positive control never delivers (1 cause)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: subscription-filter delivery to a Lambda destination never happens, so the no-match assertion can never be reached ===
AWS: a subscription filter with a non-matching filterPattern delivers nothing, while the check's positive control (a matching event on an identically configured filter) does invoke the function and lands its effect on the SQS queue; the check polls for that control first.
Emulator: the check is terminated with 'Error: Check timed out after 60.0s' — no delivery ever occurs, so the positive control's poll expires and the negative assertion is never evaluated.
Why: the filter-pattern gate is ungraded and log-driven pipelines are inert: a client cannot distinguish "the pattern correctly excluded this event" from "nothing is ever delivered".
Fix: the subscription-filter delivery path for Lambda destinations (evaluate the filter pattern on ingest and invoke the destination for matching batches).
Traces: lambda__logs__sqs.subscription-filter-non-matching-pattern-not-delivered.

## Evidence
Failing check lambda__logs__sqs.subscription-filter-non-matching-pattern-not-delivered (verdict: mismatch). Replay detail: 'Error: Check timed out after 60.0s'. Expected (AWS): the matching positive control reaches the function (and its message reaches the queue) while the non-matching event does not. Actual: neither is ever delivered, so the poll runs to the deadline. The sibling check subscription-filter-delivers-log-events-to-lambda is owned by 20261024T150703Z-lambda-logs-sqs-a-cloudwatch-logs-subscription-filter.md; this check's replay key is not in it.

## Steps to Reproduce
1. Create a function that writes to an SQS queue; AddPermission for logs.amazonaws.com.
2. logs CreateLogGroup/CreateLogStream; PutSubscriptionFilter with filterPattern="MATCHME".
3. PutLogEvents with a matching line (positive control) and with a non-matching line; poll the queue.
   Observed: no delivery at all; the check is killed at 60 s.
   Expected: exactly one delivery, for the matching line.
