Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-control-registry-record-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-control-registry-record-lifecycle
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|GetRegistryRecord|2xx->2xx|$.createdAt
Issue-Key: agent-registry-control|GetRegistryRecord|2xx->2xx|$.updatedAt
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# AgentRegistryControl serializes date-time timestamps as epoch numbers

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRegistryRecord

## Description
1 cause.

Cause 1 of 1 - registry-record timestamps are emitted as JSON numbers instead of RFC3339 date-time strings (fixture agent-registry-control-registry-record-lifecycle, step task#4).
AWS: Smithy: GetRegistryRecordResponse.createdAt and .updatedAt both target com.amazonaws.agentregistrycontrol#DateTimestamp, a timestamp shape carrying @timestampFormat("date-time"), and both are @required. restJson1's default epoch-seconds payload encoding is therefore overridden: AWS serializes these members as RFC3339/ISO-8601 date-time STRINGS (e.g. "2026-11-04T02:15:00.212Z"). The recorded response asserts the shape of these members, not a literal instant.
Emulator: emits them as bare JSON numbers - fractional epoch seconds such as 1793768100.2126694 - so the replay reports $.createdAt and $.updatedAt as value_mismatch against the shaped expectation on an otherwise correct 200 response.
Why: an AWS SDK deserializing a @timestampFormat("date-time") member expects a string; a numeric value is either rejected as a parse error or silently read as a different instant, so every client that reads a record's creation or update time breaks. The same two members appear on every record-shaped response (GetRegistryRecord, ListRegistryRecords, Create/Update responses), so the defect is service-wide rather than specific to one operation.
Fix: the timestamp serializer used by the agentregistrycontrol service crate's response serialization for DateTimestamp members - honour @timestampFormat("date-time") instead of falling back to the restJson1 epoch-seconds default.
Traces: agent-registry-control-registry-record-lifecycle#task#4 (the cluster whose only diverging keys are the two timestamps). The same numeric encoding also shows up as extra diverging keys in agent-registry-control-registry-record-lifecycle#task#6, #task#7 and agent-registry-control-registry-record-approval-workflow#task#6, #task#8, which are filed under their own root causes.

## Evidence
Replay of simparity-wire-aws-v1, cluster service=agent-registry-control|GetRegistryRecord|status=200->200, fixture agent-registry-control-registry-record-lifecycle step task#4: HTTP 200 as expected, but
  $.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1793768100.2126694)
  $.updatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1793768100.71267)
The two numbers are this run's clock values; the requirement is the WIRE TYPE - an RFC3339 date-time string - not any particular instant.
Model grounding: Smithy DateTimestamp = {"type": "timestamp", "timestampFormat": "date-time"}; GetRegistryRecordResponse.createdAt/.updatedAt target it and are @required.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. CreateRegistry, then CreateRegistryRecord in it, then GET /registries/{registryId}/records/{recordId} (fixture agent-registry-control-registry-record-lifecycle, step task#4).
3. Observe createdAt/updatedAt in the 200 body are JSON numbers; AWS returns RFC3339 date-time strings for these @timestampFormat("date-time") members.
