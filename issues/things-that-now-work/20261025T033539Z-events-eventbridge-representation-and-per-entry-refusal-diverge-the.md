Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.default-event-bus-listed events.e2e-partner-event-source-activation-to-ingestion-flow
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.default-event-bus-listed|run#10|AssertionError|assert "CreationTime" not in default_bus, (
Issue-Key: events.e2e-partner-event-source-activation-to-ingestion-flow|run#16|AssertionError|assert (entry.get("ErrorCode"), entry.get("ErrorMessage")) == (
Issue-Commit: c1baf0c9e866af3415fa02f273afb790d3f4a77b

# EventBridge representation and per-entry refusal diverge: the default event bus is listed with CreationTime/LastModifiedTime, and PutPartnerEvents rejects a non-whitelisted source with the wrong per-entry ErrorCode (2 causes)

> _Note: this issue was identified against the code base at commit `c1baf0c9e866af3415fa02f273afb790d3f4a77b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEventBuses, PutPartnerEvents

## Description
Two independent net-new EventBridge divergences, both reached on an emulator that answered the calls (no stub, no timeout). Neither is covered by the open EventBridge issues: 20261025T002357Z-events-eventbridge-refusal-codes-and-messages-diverge-from-aws.md covers top-level refusal codes/messages (not a PutPartnerEvents per-ENTRY result), and 20261025T002357Z-events-eventbridge-state-and-representation-diverge-describeconnection-leaks-the.md covers DescribeConnection/ARN-namespace representation (not the default bus's timestamps).

Cause 1 of 2 — ListEventBuses stamps the implicit default event bus with CreationTime and LastModifiedTime.
AWS: Smithy: ListEventBuses output EventBuses -> EventBusList of EventBus; EventBus.CreationTime ("The time the event bus was created.") and EventBus.LastModifiedTime are OPTIONAL members, and the model states no condition under which they are populated. Recorded behavior only: the account's default event bus is never explicitly created, so its ListEventBuses entry carries Name and an account/region-scoped Arn and NO CreationTime (and no LastModifiedTime), which is exactly what distinguishes it from a customer-created bus in the same list.
Emulator: the default entry comes back as {'Name': 'default', 'Arn': 'arn:aws:events:<region>:<account>:event-bus/default', 'CreationTime': <timestamp>, 'LastModifiedTime': <timestamp>} — both timestamps present, and equal to each other (apparently the moment the emulator seeded its state, not any customer action).
Why: the presence of CreationTime is the documented signal a client uses to tell the implicit default bus from a bus it (or another principal) created. Inventory, drift and cleanup tooling that filters "buses created by us" on CreationTime will now pick up the default bus and attempt to delete or re-tag it, and anything that reports bus age reports a fabricated creation instant that changes on every emulator restart.
Fix: the default bus's seeded state in the eventbridge service crate plus the ListEventBuses response serializer — the default bus record must hold no creation/last-modified timestamps, and the serializer must omit absent timestamps rather than defaulting them to "now". Customer-created buses must keep both fields.
Traces: events.default-event-bus-listed.

Cause 2 of 2 — PutPartnerEvents on a source the caller is not whitelisted for fails the entry with ErrorCode NotAuthorizedForSourceException instead of InvalidArgument.
AWS: Smithy: PutPartnerEvents models only InternalException (500) and OperationDisabledException (400) as top-level errors, so a caller that is not whitelisted for the named partner namespace is reported per ENTRY, via PutPartnerEventsResultEntry.ErrorCode/ErrorMessage with FailedEntryCount=1 and no EventId. Recorded behavior: ErrorCode is "InvalidArgument" and the message is the validation form built from the offending input member and the reason — e.g. "Parameter Source is not valid. Reason: Caller AWS account is not whitelisted for the source." The requirement is the ErrorCode value "InvalidArgument" plus a message naming the input member (Source) and the not-whitelisted reason; the quoted sentence is the recorded example of that rule.
Emulator: the entry comes back as {'ErrorCode': 'NotAuthorizedForSourceException', 'ErrorMessage': 'Not authorized for the source.'} — an exception-shaped code that appears nowhere in this operation's model, and a message that names neither the member nor the reason. (The rest of the same flow is already right: CreatePartnerEventSource is refused with ValidationException and DescribeEventSource then reports ResourceNotFoundException, so only the per-entry code/message diverges.)
Why: PutPartnerEvents never raises for this case, so clients branch exclusively on entry ErrorCode. Code that retries or re-queues on InvalidArgument-vs-throttling classification sees an unknown code, and partner-ingestion pipelines that assert the documented InvalidArgument contract treat the batch as an unclassified failure instead of a permanent per-entry rejection.
Fix: the PutPartnerEvents handler's per-entry source validation in the eventbridge service crate — emit the per-entry InvalidArgument code with the parameter-and-reason message form, rather than minting an exception-style code.
Traces: events.e2e-partner-event-source-activation-to-ingestion-flow.

## Evidence
Cause 1 (events.default-event-bus-listed, mismatch):
  expected: the ListEventBuses entry for 'default' has Name + account/region-scoped Arn and NO CreationTime
  actual:   AssertionError: the default bus was never explicitly created and must carry no CreationTime, got {'Name': 'default', 'Arn': 'arn:aws:events:us-west-2:000000000000:event-bus/default', 'CreationTime': datetime.datetime(2026, 10, 25, 3, 0, 17, 129293, tzinfo=tzlocal()), 'LastModifiedTime': datetime.datetime(2026, 10, 25, 3, 0, 17, 129293, tzinfo=tzlocal())}
  (Name and Arn asserts passed; only the timestamp presence diverged. The timestamp value itself is environment-specific and is not the requirement — its PRESENCE is.)

Cause 2 (events.e2e-partner-event-source-activation-to-ingestion-flow, mismatch):
  expected entry: FailedEntryCount 1, no EventId, ErrorCode 'InvalidArgument', ErrorMessage 'Parameter Source is not valid. Reason: Caller AWS account is not whitelisted for the source.'
  actual entry:   AssertionError: {'ErrorCode': 'NotAuthorizedForSourceException', 'ErrorMessage': 'Not authorized for the source.'}
  Preceding steps in the same check passed: CreatePartnerEventSource -> ValidationException, DescribeEventSource -> ResourceNotFoundException.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and, with no prior setup, call events:ListEventBuses (paging NextToken to exhaustion).
2. Find the entry whose Name == 'default'.
3. Observe CreationTime and LastModifiedTime present on it; AWS returns neither for the implicit default bus.

Cause 2:
1. source_name = 'aws.partner/<unique-partner>/<unique-generator>' for a namespace this account is not whitelisted for.
2. events:CreatePartnerEventSource(Name=source_name, Account=<caller account>) -> refused with ValidationException (already correct).
3. events:PutPartnerEvents(Entries=[{Time: now, Source: source_name, DetailType: 'x', Detail: '{"ok": true}'}]).
4. Inspect Entries[0]: emulator yields ErrorCode 'NotAuthorizedForSourceException'; AWS yields ErrorCode 'InvalidArgument' with a message naming parameter Source and the not-whitelisted reason.

Replay: run_behavioral_suite and re-run checks events.default-event-bus-listed and events.e2e-partner-event-source-activation-to-ingestion-flow.
