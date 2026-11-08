Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__logs.xsvc-logs-1-custom-bus-rule-writes-the-exact-event-envelope-to-a-log-group events__logs.xsvc-logs-1-log-group-without-events-policy-not-delivered-and-counts-failed-invocations
Issue-Key: events__logs.xsvc-logs-1-custom-bus-rule-writes-the-exact-event-envelope-to-a-log-group|_emit_until#13|AssertionError|raise AssertionError(f"nothing observed after {attempts} PutEvents attempts (sent {sent!r})")
Issue-Key: events__logs.xsvc-logs-1-log-group-without-events-policy-not-delivered-and-counts-failed-invocations|_emit_until#14|AssertionError|raise AssertionError(f"nothing observed after {attempts} PutEvents attempts (sent {sent!r})")
Issue-Commit: 8f24345936df6262f616e6a72be898a454a52720

# events__logs: a rule whose target is a CloudWatch Logs group writes no log record at all, so neither the envelope positive nor the resource-policy negative can be observed

> _Note: this issue was identified against the code base at commit `8f24345936df6262f616e6a72be898a454a52720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
ONE root cause (Cause 1 of 1), grounded in two failing checks.

Neither failing check is marked ALREADY FILED or OWNED in the replay report. The open issues 20261107T205257Z-events-logs-2-net-new-causes-an-event.md and 20261104T104531Z-events-logs-an-eventbridge-cloudwatch-logs-target-never.md describe the same symptom for other checks but do not carry these checks' replay keys, so they do not cover them.

Cause 1 of 1 — an EventBridge rule with a CloudWatch Logs target never delivers the matched event to the log group.
AWS: Smithy (eventbridge): PutRule + PutTargets accept a Target whose Arn is a log-group ARN; PutEvents returns a PutEventsResultEntry carrying EventId for each accepted entry (FailedEntryCount 0). Recorded behavior: for a matching entry, EventBridge writes ONE log event into the target log group whose message is the event envelope as JSON — the fields version, id (the EventId PutEvents returned), detail-type, source, account, time, region, resources and detail, with detail byte-identical to the Detail the producer sent. Delivery to a log group that carries no resource policy for delivery.logs.amazonaws.com/events.amazonaws.com is REFUSED by the service: nothing is written to that group and the rule's FailedInvocations metric counts the attempt, while a sibling group that does carry the policy still receives the record.
Emulator: PutEvents is accepted (an EventId is minted for every entry) but nothing is ever written to the target log group. Both checks retry PutEvents eight times and then fail on their positive control with "AssertionError: nothing observed after 8 PutEvents attempts (sent [[<event id>], ... x8])" — the per-run event ids are the only variable part. The rule and target are accepted and have no delivery behind them.
Why: a client that uses a CloudWatch Logs target as the audit record of a bus reads an empty log group and cannot distinguish "no event matched" from "delivery is not implemented". The permission-chain negative is equally unobservable: because the permitted control group receives nothing either, a missing log-group resource policy cannot be shown to be the thing that blocks delivery, and no FailedInvocations evidence is produced.
Fix: the EventBridge rule-dispatch path's CloudWatch Logs target — serialize the matched event envelope and PutLogEvents it into the target group, authorize the write against the group's resource policy, and count a blocked delivery as a failed invocation.
Traces: events__logs.xsvc-logs-1-custom-bus-rule-writes-the-exact-event-envelope-to-a-log-group, events__logs.xsvc-logs-1-log-group-without-events-policy-not-delivered-and-counts-failed-invocations.

## Evidence
Expected: after PutEvents of one entry matching the rule (source "custom.simparity", detail-type "OrderPlaced"), FilterLogEvents on the target log group returns one event whose message is the event envelope JSON, with id equal to the EventId PutEvents returned and detail equal to the Detail sent; for the negative, the group without the events resource policy stays empty while the permitted twin receives the record and the rule's FailedInvocations counts the blocked attempt.
Actual (both checks): `AssertionError: nothing observed after 8 PutEvents attempts (sent [['18e5e6e4-33cf-d238-0000-000000000001'], ... eight ids ...])` — PutEvents accepted every entry; the target log group received nothing at all, including the permitted control group of the negative check.

## Steps to Reproduce
1. logs.create_log_group(<group>) and put a resource policy allowing the EventBridge delivery principal logs:PutLogEvents/CreateLogStream on it.
2. events.create_event_bus(<bus>); events.put_rule with an EventPattern on source "custom.simparity" and detail-type "OrderPlaced"; events.put_targets with the log group's ARN.
3. events.put_events of one matching entry (it is accepted, an EventId is returned).
4. Poll logs.filter_log_events on the group — no event ever arrives (repeat up to 8 times).
5. Repeat 1-4 with a second group that has NO events resource policy and compare: the permitted group is empty too, so the refusal cannot be attributed.
