Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.put-events-nonexistent-bus-silently-dropped events.put-events-one-mebibyte-request-limit-refused-whole-and-large-event-delivers-intact events.put-events-detail-json-nesting-depth-limit events.remove-targets-unknown-id-is-lenient
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.put-events-nonexistent-bus-silently-dropped|run#4|AssertionError|assert resp.get("FailedEntryCount", 0) == 0, (
Issue-Key: events.put-events-one-mebibyte-request-limit-refused-whole-and-large-event-delivers-intact|run#11|AssertionError|assert at["FailedEntryCount"] == 0 and at["Entries"][0].get("EventId"), (
Issue-Key: events.put-events-detail-json-nesting-depth-limit|run#11|AssertionError|assert _DOC_DEPTH - 1 <= depth <= _DOC_DEPTH + 1, (
Issue-Key: events.remove-targets-unknown-id-is-lenient|run#8|AssertionError|assert resp["FailedEntryCount"] == 0, f"unknown Id must not be reported as failed: {resp}"
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# PutEvents/RemoveTargets entry-level semantics diverge: a nonexistent bus is reported as a failed entry, a 1 MiB entry is refused, detail nesting is capped at 127 levels, and RemoveTargets is not lenient (4 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Four independent divergences in how per-entry results and per-entry limits are computed.

Cause 1 of 4 — PutEvents to a nonexistent bus is reported as a failed entry instead of being accepted and dropped.
AWS: PutEvents' only modeled error is InternalException/500; a bus that does not exist is not a client error. The entry is ACCEPTED (FailedEntryCount 0, an EventId is issued) and the event is simply never routed — EventBridge does not tell the publisher that the destination bus is missing.
Emulator: returns FailedEntryCount 1 with `{'ErrorCode': 'ResourceNotFoundException', 'ErrorMessage': 'Event bus <name> does not exist.'}`.
Why: publishers key on FailedEntryCount to decide whether to retry or alarm. A client publishing to a bus created later now sees permanent per-entry failures and retries forever, or treats a normal condition as an outage.
Fix: bus resolution and entry-result construction in services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs.
Traces: events.put-events-nonexistent-bus-silently-dropped.

Cause 2 of 4 — an entry of exactly the maximum size is refused (off-by-one on the size limit).
AWS: the per-entry/per-request size limit is 1 MiB = 1048576 bytes INCLUSIVE; an entry of exactly 1048576 bytes is accepted and delivers intact. Only a larger one is refused, and an oversized request is refused as a whole.
Emulator: refuses the exactly-1 MiB entry with `{'ErrorCode': 'InvalidArgument', 'ErrorMessage': 'Total size of the entries in the request is 1048576 bytes, which exceeds the maximum entry size of 1048576 bytes.'}` — its own message states 1048576 is not greater than 1048576, so the comparison is `>=` where it must be `>`.
Why: clients that size payloads to the documented maximum have every such event rejected.
Fix: the size comparison in put_events.rs.
Traces: events.put-events-one-mebibyte-request-limit-refused-whole-and-large-event-delivers-intact.

Cause 3 of 4 — the accepted JSON nesting depth of `Detail` is 127, not the documented ~1000.
AWS: the documented maximum nesting depth for a PutEvents detail is 1000 levels (the check allows +/-1 for whether the outermost container counts); this holds for objects, arrays and mixed nesting alike.
Emulator: the measured maximum is 127 for all three shapes — a parser-depth artefact, not the service limit.
Why: a client with legitimately deep event detail (a nested document, a serialized tree) has its events rejected by a limit ~8x stricter than AWS's.
Fix: the JSON parse/validate depth bound used by put_events.rs.
Traces: events.put-events-detail-json-nesting-depth-limit.

Cause 4 of 4 — RemoveTargets reports an unknown target Id as a failed entry instead of being lenient.
AWS: RemoveTargets is idempotent/lenient about Ids that are not attached: FailedEntryCount 0 and no FailedEntries. Its modeled errors are ConcurrentModificationException, InternalException, ManagedRuleException and ResourceNotFoundException (for the rule/bus, not for an Id).
Emulator: returns FailedEntryCount 1 with `{'TargetId': 'never-existed', 'ErrorCode': 'ResourceNotFoundException', 'ErrorMessage': 'Rule <rule> has no target with Id never-existed.'}`.
Why: teardown and reconciliation loops that remove a known set of Ids now fail whenever a target was already gone, breaking idempotent cleanup.
Fix: entry-result construction in .../src/ops/remove_targets.rs.
Traces: events.remove-targets-unknown-id-is-lenient.

## Evidence
1. PutEvents to a never-created bus: "expected no failures for nonexistent-bus entry, got {'FailedEntryCount': 1, 'Entries': [{'ErrorCode': 'ResourceNotFoundException', 'ErrorMessage': 'Event bus sim2real-behav-evb-no-such-bus-<id> does not exist.'}], 'HTTPStatusCode': 200}". Expected: FailedEntryCount 0 and an EventId.
2. PutEvents with an entry of exactly 1048576 bytes: "an entry of exactly 1048576 bytes (1 MiB) must be accepted, got [{'ErrorCode': 'InvalidArgument', 'ErrorMessage': 'Total size of the entries in the request is 1048576 bytes, which exceeds the maximum entry size of 1048576 bytes.'}]".
3. PutEvents nesting-depth probe: "the maximum accepted nesting depth must be the documented 1000 levels (+/-1 ...); measured 127 across shapes {'objects': 127, 'arrays': 127, 'mixed': 127}".
4. RemoveTargets with an Id that was never attached: "unknown Id must not be reported as failed: {'FailedEntryCount': 1, 'FailedEntries': [{'TargetId': 'never-existed', 'ErrorCode': 'ResourceNotFoundException', 'ErrorMessage': 'Rule <rule> has no target with Id never-existed.'}], 'HTTPStatusCode': 200}". Expected: FailedEntryCount 0, FailedEntries [].

## Steps to Reproduce
1. PutEvents(Entries=[{Source:'x', DetailType:'t', Detail:'{}', EventBusName:'no-such-bus'}]) -> observe FailedEntryCount 1.
2. PutEvents with a single entry padded so the serialized entry is exactly 1048576 bytes -> observe InvalidArgument.
3. PutEvents with Detail nested 200 / 500 / 999 levels deep -> observe refusal beyond 127.
4. PutRule(Name=r)+PutTargets(Rule=r, Targets=[{Id:'t1',...}]); RemoveTargets(Rule=r, Ids=['never-existed']) -> observe FailedEntryCount 1.
