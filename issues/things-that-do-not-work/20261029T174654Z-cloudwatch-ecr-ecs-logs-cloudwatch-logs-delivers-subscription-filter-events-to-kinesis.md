Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver
Issue-Key: cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver|run#10|AssertionError|assert not delivered, (
Issue-Commit: b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7

# CloudWatch Logs delivers subscription-filter events to Kinesis through a role with no kinesis:PutRecord permission

> _Note: this issue was identified against the code base at commit `b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
ONE net-new root cause.

Cause 1 of 1 — a subscription filter whose roleArn cannot write to the destination still delivers log events.
AWS: recorded behavior — CloudWatch Logs validates the delivery role's permission on the Kinesis destination when the subscription filter is created, so either PutSubscriptionFilter is refused with InvalidParameterException (the role cannot be assumed / lacks access to the destination) or, if creation is accepted, zero log events are ever delivered. A role whose trust policy allows logs.amazonaws.com but whose identity policy grants no kinesis:PutRecord can never put a record, so no event can appear in the stream.
Emulator: PutSubscriptionFilter is accepted, and a PutLogEvents-written event IS delivered into the Kinesis stream through that permission-less role — the emulator performs no authorization on the delivery path (neither at filter-creation time nor at delivery time).
Why: the failure mode an operator most needs to reproduce (a subscription filter that silently delivers nothing because its role is under-privileged) is impossible to reproduce here; worse, the emulator is over-permissive, so a configuration that cannot work in AWS appears to work, and a test suite that validates least-privilege IAM for a log-delivery pipeline passes against the emulator and fails in production.
Fix: the PutSubscriptionFilter validation path (resolve the roleArn and check kinesis:PutRecord on the destination, refusing with InvalidParameterException when absent) and the log-event delivery path, which must evaluate the role's permissions before writing to the destination.
Traces: cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver.

## Evidence
[mismatch] cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver, assertion at check line 49:
"AssertionError: log event was delivered to Kinesis via a role (arn:aws:iam::000000000000:role/sim2real-behav-cwl2k-...) with no kinesis:PutRecord permission — the subscription filter should have failed to deliver".
The check accepts EITHER a refused PutSubscriptionFilter (InvalidParameterException) OR zero delivery; the emulator both accepted the filter and delivered the record. The role name is a per-run value; the requirement is that a role with no kinesis:PutRecord permission delivers nothing.

## Steps to Reproduce
1) logs CreateLogGroup(<group>). 2) kinesis CreateStream(<stream>). 3) iam CreateRole with a trust policy allowing logs.amazonaws.com to sts:AssumeRole and NO identity policy granting kinesis:PutRecord. 4) logs PutSubscriptionFilter(logGroupName=<group>, destinationArn=<stream arn>, roleArn=<that role>, filterPattern=""). 5) logs PutLogEvents with a unique marker into a stream of <group>. 6) Read the Kinesis stream: the marker record is present, and step 4 was not refused.
