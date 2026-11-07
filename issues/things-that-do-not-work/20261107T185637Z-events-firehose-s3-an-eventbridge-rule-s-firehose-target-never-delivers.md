Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets
Issue-Key: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets|_deliver_until_firehose_receives#11|AssertionError|raise AssertionError(
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# An EventBridge rule's Firehose target never delivers, so fan-out to Firehose produces no S3 records

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issue 20261029T175617Z-events-firehose-s3-an-eventbridge-rule-s-firehose.md owns other checks of this service and does not carry this replay key.

Cause 1 of 1: a rule with both a Firehose and an SQS target delivers nothing to the Firehose leg.
AWS: PutEvents of an event matching the rule invokes every target independently. For a Kinesis Data Firehose target, EventBridge assumes the target's RoleArn and calls PutRecord with the (optionally input-transformed) event as the record; Firehose then buffers and delivers the record to its S3 destination, where the event JSON — carrying the marker from the request's detail — is readable under the destination prefix.
Emulator: nothing reaches S3 through the Firehose target. The check polls 140 s and reports "event delivered to S3 via the Firehose target not met within 140.0s; last marker '<marker>', records seen: []" — the empty records list is the defect; the marker is this run's value.
Why: EventBridge-to-Firehose fan-out — the standard way to archive events — silently drops every event. A caller sees PutEvents succeed with FailedEntryCount 0 and no data anywhere, and a multi-target rule cannot be shown to deliver independently.
Fix: the EventBridge target dispatcher's Firehose target type (role assumption and the PutRecord call), and/or the Firehose DirectPut ingestion-to-S3 delivery path it feeds.
Traces: events__firehose__s3.eventbridge-fanout-firehose-and-sqs-targets.

## Evidence
Expected: the event JSON delivered through the Firehose target lands as an S3 object under the destination prefix. Actual: no record ever appears — 'AssertionError: event delivered to S3 via the Firehose target not met within 140.0s; last marker ..., records seen: []'.

## Steps to Reproduce
1) Create a Firehose delivery stream with an S3 destination and a role that can write it.
2) Create an EventBridge rule with two targets: the delivery stream (with a target role) and an SQS queue.
3) PutEvents a matching event, then list the Firehose destination prefix for the event JSON.
