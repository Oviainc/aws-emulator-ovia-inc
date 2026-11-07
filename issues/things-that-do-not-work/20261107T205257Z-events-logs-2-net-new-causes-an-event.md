Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists events__logs.xsvc-logs-1-bus-logging-include-detail-none-omits-the-event-detail events__logs.xsvc-logs-1-error-level-bus-log-records-the-no-permissions-invocation-failure events__logs.xsvc-logs-1-input-template-without-timestamp-and-message-not-delivered-to-log-group events__logs.xsvc-logs-1-input-transformer-renders-message-and-event-time-into-the-log-record events__logs.xsvc-logs-1-near-miss-detail-type-rule-not-delivered-to-its-log-group
Issue-Key: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists|_emit_until#18|AssertionError|raise AssertionError(f"nothing observed after {attempts} PutEvents attempts (sent {sent!r})")
Issue-Key: events__logs.xsvc-logs-1-bus-logging-include-detail-none-omits-the-event-detail|run#11|AssertionError|assert done, (
Issue-Key: events__logs.xsvc-logs-1-error-level-bus-log-records-the-no-permissions-invocation-failure|run#18|AssertionError|assert found, f"no INVOCATION_FAILURE record for EventIds {sorted(sent)!r} in {log_group}"
Issue-Key: events__logs.xsvc-logs-1-input-template-without-timestamp-and-message-not-delivered-to-log-group|_emit_until#13|AssertionError|raise AssertionError(f"nothing observed after {attempts} PutEvents attempts (sent {sent!r})")
Issue-Key: events__logs.xsvc-logs-1-input-transformer-renders-message-and-event-time-into-the-log-record|_emit_until#9|AssertionError|raise AssertionError(f"nothing observed after {attempts} PutEvents attempts (sent {sent!r})")
Issue-Key: events__logs.xsvc-logs-1-near-miss-detail-type-rule-not-delivered-to-its-log-group|_emit_until#12|AssertionError|raise AssertionError(f"nothing observed after {attempts} PutEvents attempts (sent {sent!r})")
Issue-Commit: 959ef406fada22402fcbc8404625fdf48db790f8

# events__logs: 2 net-new causes — an event bus's LogConfig writes no vended-log records at all, and a rule's CloudWatch Logs target never writes a log event

> _Note: this issue was identified against the code base at commit `959ef406fada22402fcbc8404625fdf48db790f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventBus / PutTargets / PutEvents

## Description
Two causes under the events__logs heading; the group is 0/8. Related open issue that does not cover these checks: 20261104T104531Z-events-logs-an-eventbridge-cloudwatch-logs-target-never.md.

Cause 1 of 2: an event bus configured with LogConfig emits no vended-log records once a CloudWatch Logs delivery exists.
AWS: Smithy LogConfig{Level: OFF|ERROR|INFO|TRACE, IncludeDetail: NONE|FULL} on UpdateEventBus/DescribeEventBus ("The logging configuration settings for the event bus"). With the bus registered as a delivery source (PutDeliverySource service=events, resourceArns=[bus arn]) and a CreateDelivery to a /aws/vendedlogs log group, every PutEvents at Level=INFO yields an EVENT_INGEST_SUCCESS record carrying resource_arn, event_bus_name, log_level and a details object with source, detail_type, caller_account_id and — when IncludeDetail=FULL — event_detail round-tripping the put detail, which is absent when IncludeDetail=NONE; at Level=ERROR a target invocation refused for permissions yields an INVOCATION_FAILURE record.
Emulator: UpdateEventBus and DescribeEventBus echo LogConfig correctly (those assertions pass), but no record is ever written: "nothing observed after 8 PutEvents attempts", "ingest records missing: FULL=0 NONE=0 for EventIds {...}", "no INVOCATION_FAILURE record for EventIds [...] in /aws/vendedlogs/<group>". The log group names and event ids are per-run values; the missing record types (EVENT_INGEST_SUCCESS, INVOCATION_FAILURE) and fields are the requirement.
Failing checks: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists, events__logs.xsvc-logs-1-bus-logging-include-detail-none-omits-the-event-detail, events__logs.xsvc-logs-1-error-level-bus-log-records-the-no-permissions-invocation-failure.

Cause 2 of 2: a rule whose target is a CloudWatch Logs log group never writes any log event.
AWS: a rule with a log-group target (the group's resource policy admitting events.amazonaws.com for the rule ARN) writes one log event per matching event; with an InputTransformer the rendered template supplies the record's timestamp/message, a template lacking them is rejected at delivery rather than written, and a near-miss detail-type is simply not delivered — in each case the positive leg writes something to the group.
Emulator: nothing is ever written to the log group, so neither the positive leg nor any negative leg can be established: "nothing observed after 8 PutEvents attempts (sent [...])" in all three checks.
Failing checks: events__logs.xsvc-logs-1-input-template-without-timestamp-and-message-not-delivered-to-log-group, events__logs.xsvc-logs-1-input-transformer-renders-message-and-event-time-into-the-log-record, events__logs.xsvc-logs-1-near-miss-detail-type-rule-not-delivered-to-its-log-group.

Why: a client that turns on bus logging for troubleshooting sees an empty log group and cannot tell an ingest from a dropped event or a failed invocation (cause 1); and every archive/observability pipeline that fans events into CloudWatch Logs through a rule target records nothing at all (cause 2).

Fix: the event bus's log-emission path (it must write EVENT_INGEST_SUCCESS/INVOCATION_FAILURE records honouring Level and IncludeDetail to the configured CloudWatch Logs delivery) and the EventBridge target handler for a logs log-group ARN (it must PutLogEvents the rendered input, honouring the input transformer and refusing a template without timestamp/message).

Traces: the six failing check ids in trace_ids.

## Evidence
Cause 1 — Expected: an EVENT_INGEST_SUCCESS record per PutEvents with resource_arn/event_bus_name/log_level=INFO and details.event_detail present under FULL and absent under NONE; an INVOCATION_FAILURE record at Level=ERROR. Actual: AssertionError: nothing observed after 8 PutEvents attempts (sent [...]); AssertionError: ingest records missing: FULL=0 NONE=0 for EventIds {...}; AssertionError: no INVOCATION_FAILURE record for EventIds [...] in /aws/vendedlogs/<group>.
Cause 2 — Expected: one log event in the target group per matching PutEvents. Actual: AssertionError: nothing observed after 8 PutEvents attempts (sent [...]) in all three log-group-target checks.

## Steps to Reproduce
Cause 1: CreateEventBus; UpdateEventBus Name=<bus> LogConfig={Level: INFO, IncludeDetail: FULL}; create a /aws/vendedlogs/<name> log group with a delivery resource policy; PutDeliverySource(service=events, resourceArns=[bus arn]); PutDeliveryDestination + CreateDelivery to the group; PutEvents repeatedly; FilterLogEvents the group. Repeat with IncludeDetail=NONE and with Level=ERROR plus a target whose invocation is refused for permissions.
Cause 2: CreateLogGroup; put a resource policy admitting events.amazonaws.com for the rule ARN; PutRule + PutTargets with the log-group ARN (with and without an InputTransformer); PutEvents matching and near-miss events; FilterLogEvents the group.
