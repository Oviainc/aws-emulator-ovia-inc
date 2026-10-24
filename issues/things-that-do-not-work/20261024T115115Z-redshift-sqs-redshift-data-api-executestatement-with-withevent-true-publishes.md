Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.data-api-with-event-emits-a-real-statement-event-and-the-default-emits-none
Issue-Key: redshift__sqs.data-api-with-event-emits-a-real-statement-event-and-the-default-emits-none|<lambda>#28|Timeout|lambda: sqs.receive_message(
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Redshift Data API ExecuteStatement with WithEvent=true publishes no EventBridge statement-status event, so no event ever reaches an SQS target (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
One net-new root cause.

=== Cause 1 of 1: WithEvent=true emits nothing — the statement-status-change event is never published ===
Failing check: redshift__sqs.data-api-with-event-emits-a-real-statement-event-and-the-default-emits-none

AWS: Smithy (redshift-data) ExecuteStatement input member WithEvent (Boolean), documented as "A value that indicates whether to send an event to the Amazon EventBridge event bus after the SQL statement runs." With WithEvent=true, the Data API publishes an event on the default bus with source "aws.redshift-data" and detail-type "Redshift Data Statement Status Change", whose detail carries the statementId and state; an EventBridge rule matching on source + detail-type delivers that envelope to its target (here an SQS queue). WithEvent defaults to false, so an otherwise identical statement submitted without the field publishes nothing.

Emulator: the check produces no verdict — 'Check timed out after 60.0s'. The positive leg (submit with WithEvent=true, then drain the SQS target for an envelope keyed by that statementId) never completes, so the measured latency the negative leg is calibrated against is never obtained. Recorded behavior only for the exact stall point; the replay output carries no frame. Hypothesis: the Data API never publishes to EventBridge at all, so the rule's SQS target stays empty and the check polls to the cap.

Why: a client that relies on WithEvent to drive asynchronous completion handling — an EventBridge rule fanning statement-status changes to a queue, a Lambda, or a step function — receives nothing and waits forever; there is no other signal that distinguishes 'WithEvent was honoured' from 'WithEvent was silently ignored', and an emulator that emits nothing also trivially 'passes' the negative half of the contract (that WithEvent-less statements emit nothing).

Fix: the redshift-data ExecuteStatement handler and the statement state machine must publish a "Redshift Data Statement Status Change" event under source "aws.redshift-data" to the default EventBridge bus when WithEvent is true (and only then), with detail.statementId and detail.state set, so existing EventBridge rule-to-SQS routing delivers it.

Traces: redshift__sqs.data-api-with-event-emits-a-real-statement-event-and-the-default-emits-none

## Evidence
Expected: redshift-data ExecuteStatement(..., WithEvent=True) on an available cluster causes an EventBridge envelope with source 'aws.redshift-data' and detail-type 'Redshift Data Statement Status Change' to be delivered to the SQS queue targeted by a rule matching that source and detail-type, keyed by detail.statementId; the twin statement submitted without WithEvent produces no message, and GetQueueAttributes reports ApproximateNumberOfMessages '0' after draining. Actual: 'Check timed out after 60.0s' — the positive statement's event never arrives, so the run yields no delivery verdict at all.

## Steps to Reproduce
1) Create an SQS queue and read its QueueArn; put an SQS access policy allowing events.amazonaws.com. 2) events PutRule with EventPattern {"source":["aws.redshift-data"],"detail-type":["Redshift Data Statement Status Change"]}, State=ENABLED, and PutTargets pointing at the queue. 3) redshift CreateCluster (single-node) and wait for available. 4) redshift-data ExecuteStatement(ClusterIdentifier=<cluster>, Database=devdb, DbUser=adminuser, Sql='SELECT 1', WithEvent=True, StatementName=...); poll DescribeStatement to a terminal status. 5) ReceiveMessage on the queue and look for an envelope whose detail.statementId equals the returned Id — none arrives. 6) Submit the identical statement with WithEvent omitted and confirm nothing arrives either. Replay: redshift__sqs.data-api-with-event-emits-a-real-statement-event-and-the-default-emits-none.
