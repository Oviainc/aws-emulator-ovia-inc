Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists
Issue-Key: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists|run#29|AssertionError|assert found, f"no EVENT_INGEST_SUCCESS record for EventIds {sorted(sent)!r} in {log_group}"
Issue-Commit: 2ede6488b773bac2f5ae3900456fa42c3facff99

# events__logs: an INFO/FULL-logging event bus paired with a CloudWatch Logs delivery writes no EVENT_INGEST_SUCCESS record

> _Note: this issue was identified against the code base at commit `2ede6488b773bac2f5ae3900456fa42c3facff99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: an event bus whose LogConfig is {Level: INFO, IncludeDetail: FULL} and which is registered as a CloudWatch Logs delivery source emits no vended-log records once the delivery exists — PutEvents keeps succeeding and the rule target keeps receiving, but the delivery's log group stays empty. Failing check: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists.

Related open issue that does NOT cover this failure: 20261107T205257Z-events-logs-2-net-new-causes-an-event.md. Its cause 1 names this behavior, but the check now fails at a different point — the earlier legs it was keyed on (events flowing through the bus to the control log group, and the empty-group assertion before any delivery exists) now pass, and the replay stops later, at the missing EVENT_INGEST_SUCCESS record after CreateDelivery. The suite therefore does not mark this failure as owned; it is filed here with that issue named so the two are not closed as one.

AWS: Smithy models LogConfig{Level: OFF|ERROR|INFO|TRACE, IncludeDetail: NONE|FULL} on UpdateEventBus/DescribeEventBus ("The logging configuration settings for the event bus", documented under "Configuring logs for event buses"); the record contents are recorded behavior only. The recording shows that with PutDeliverySource(service=events, resourceArns=[<bus ARN>], logType=INFO_LOGS) and a CreateDelivery pairing that source to a /aws/vendedlogs/<name> log group, every accepted PutEvents on the bus produces one record in that group with message_type=EVENT_INGEST_SUCCESS, resource_arn=<the bus ARN>, event_bus_name=<the bus name>, log_level=INFO, and a details object carrying source, detail_type, caller_account_id=<the calling account> and, because IncludeDetail=FULL, event_detail that round-trips the Detail string passed to PutEvents. Before the delivery exists the group must hold no streams at all. Record ids, bus/group names and account are per-run values; the record type and its field set are the requirement.

Emulator: UpdateEventBus and DescribeEventBus echo {Level: INFO, IncludeDetail: FULL} correctly and the pre-delivery leg is correct (no log streams in the group), but after CreateDelivery ten successive PutEvents calls (each returning FailedEntryCount 0 with an EventId) produce no record at all in the delivery's log group: "no EVENT_INGEST_SUCCESS record for EventIds [...] in /aws/vendedlogs/<group>" after the full 10-attempt, 15s-per-attempt poll. No EVENT_INGEST_SUCCESS record exists, so none of resource_arn, event_bus_name, log_level, details.source/detail_type/caller_account_id or details.event_detail can be observed either.

Why: a client that turns bus logging on to troubleshoot a pipeline sees an empty log group and cannot distinguish an event the bus ingested from one it never received or dropped; dashboards, alarms and audit exports built on the vended EVENT_INGEST_SUCCESS stream report zero traffic on a bus that is in fact delivering.

Fix: the event bus's log-emission path — the component that, when the bus has a LogConfig at Level INFO or TRACE and a CloudWatch Logs delivery resolves from its registered delivery source, writes one EVENT_INGEST_SUCCESS record per ingested event into the delivery's log group, honouring Level and including details.event_detail only when IncludeDetail=FULL. The delivery-source/delivery lookup that joins a bus ARN to its destination log group belongs in the same path.

Traces: events__logs.xsvc-logs-1-bus-info-logging-reaches-the-log-group-only-once-a-delivery-exists.

## Evidence
Expected (AWS): after CreateDelivery pairs the bus's INFO_LOGS delivery source to the /aws/vendedlogs log group, each PutEvents yields one record in that group with message_type=EVENT_INGEST_SUCCESS, event_id=<the EventId returned by PutEvents>, resource_arn=<bus ARN>, event_bus_name=<bus name>, log_level="INFO", details.source=<Source>, details.detail_type=<DetailType>, details.caller_account_id=<calling account>, and details.event_detail round-tripping the Detail JSON.
Actual (emulator): the group contains no such record after 10 PutEvents calls, each accepted with FailedEntryCount 0. Replay detail: AssertionError: no EVENT_INGEST_SUCCESS record for EventIds ['...'] in /aws/vendedlogs/<group> (10 per-run event ids and the generated group name are environment values, not requirements).
Passing legs that localize the defect: UpdateEventBus echoed LogConfig {Level: INFO, IncludeDetail: FULL}; DescribeEventBus returned the same; PutDeliverySource bound service=events with resourceArns=[bus ARN]; events reached the rule's control log group target; DescribeLogStreams on the vended-logs group was empty before the delivery existed.

## Steps to Reproduce
1. CreateEventBus <bus>; UpdateEventBus Name=<bus> LogConfig={Level: INFO, IncludeDetail: FULL}; DescribeEventBus confirms it.
2. CreateLogGroup <ctl>; PutResourcePolicy admitting events.amazonaws.com for the rule ARN; PutRule on <bus> with EventPattern {"source":["custom.simparity"],"detail-type":["OrderPlaced"]}; PutTargets with the <ctl> group ARN (this leg works and proves events flow).
3. CreateLogGroup /aws/vendedlogs/<name> with a resource policy admitting delivery.logs.amazonaws.com; PutDeliverySource(name=<src>, logType="INFO_LOGS", resourceArn=<bus ARN>); PutDeliveryDestination for the group; CreateDelivery pairing <src> to it.
4. PutEvents on <bus> with Source=custom.simparity, DetailType=OrderPlaced, Detail={"id":...,"phase":"post","qty":2}; repeat up to 10 times.
5. FilterLogEvents on /aws/vendedlogs/<name>: no EVENT_INGEST_SUCCESS record for any returned EventId ever appears (the group has no records at all).
