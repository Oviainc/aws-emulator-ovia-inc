Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists
Issue-Key: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists|run#28|Timeout|time.sleep(3.0)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# events__logs: an INFO/FULL event-bus log configuration never writes EVENT_INGEST_SUCCESS records once a delivery exists, so the bus-logging check reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventBus

## Description
Cause 1 of 1: an event bus with LogConfig Level INFO / IncludeDetail FULL, registered as a CloudWatch Logs delivery source, never produces bus log records after CreateDelivery.
AWS: before a delivery exists the log group stays empty; after PutDeliverySource/PutDeliveryDestination/CreateDelivery wire the bus to the group, every PutEvents entry yields an EVENT_INGEST_SUCCESS record naming the bus, the EventId, the caller account and the full event detail, readable within seconds via FilterLogEvents. Recorded behavior only for the record's exact field spelling.
Emulator: the check produces no verdict at all — 'Error: Check timed out after 60.0s'; the awaited record never appears.
Why: EventBridge bus logging is the only way to see that an event was ingested and routed; with no records a customer cannot distinguish 'not configured', 'not delivered' and 'event dropped'.
Fix: the events bus log-configuration emitter and its logs vended-delivery path (delivery source/destination/delivery) that must write EVENT_INGEST_SUCCESS records into the destination log group.
Traces: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: no records before CreateDelivery, then one EVENT_INGEST_SUCCESS record per PutEvents entry carrying the bus name, EventId, caller account and the full detail. Actual: no record is ever delivered, so the check never reaches its assertions.

## Steps to Reproduce
Replay events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists: create a bus, UpdateEventBus with LogConfig {Level: INFO, IncludeDetail: FULL}, PutEvents before any delivery, then PutDeliverySource/PutDeliveryDestination/CreateDelivery to a log group, PutEvents again and FilterLogEvents the group.
