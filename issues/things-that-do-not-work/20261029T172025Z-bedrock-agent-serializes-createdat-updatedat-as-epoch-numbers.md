Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-data-source-custom-connector-lifecycle
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-data-source-custom-connector-lifecycle bedrock-agent-data-source-type-mismatch-errors bedrock-agent-delete-knowledge-base-conflict-recovery bedrock-agent-ingestion-job-lifecycle bedrock-agent-knowledge-base-documents-ingest-and-list bedrock-agent-knowledge-base-lifecycle bedrock-agent-flow-crud-lifecycle bedrock-agent-flow-prepare-and-version bedrock-agent-flow-multi-isolation
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|CreateKnowledgeBase|2xx->2xx|$.knowledgeBase.createdAt
Issue-Key: bedrock-agent|CreateKnowledgeBase|2xx->2xx|$.knowledgeBase.updatedAt
Issue-Key: bedrock-agent|GetKnowledgeBase|2xx->2xx|$.knowledgeBase.createdAt
Issue-Key: bedrock-agent|GetKnowledgeBase|2xx->2xx|$.knowledgeBase.updatedAt
Issue-Key: bedrock-agent|CreateFlow|2xx->2xx|$.createdAt
Issue-Key: bedrock-agent|CreateFlow|2xx->2xx|$.updatedAt
Issue-Key: bedrock-agent|GetFlow|2xx->2xx|$.createdAt
Issue-Key: bedrock-agent|GetFlow|2xx->2xx|$.updatedAt
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent serializes createdAt/updatedAt as epoch numbers instead of ISO-8601 date-time

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKnowledgeBase

## Description
Every bedrock-agent response that carries a resource timestamp emits it as a bare JSON number (fractional epoch seconds) where AWS emits an RFC-3339/ISO-8601 date-time string. This is a single serializer defect in the service's timestamp projection, observed on four distinct operations. 4 causes.

Cause 1 of 4: CreateKnowledgeBase (fixture bedrock-agent-data-source-custom-connector-lifecycle, step setup#2).
AWS: Smithy: KnowledgeBase.createdAt and KnowledgeBase.updatedAt target DateTimestamp with @timestampFormat("date-time") and are @required; CreateKnowledgeBase answers HTTP 202 with CreateKnowledgeBaseResponse.knowledgeBase. A date-time timestamp is serialized on the wire as an ISO-8601 string (e.g. "2026-10-29T11:38:56.074Z"), never as a number.
Emulator: answers 202 with $.knowledgeBase.createdAt and $.knowledgeBase.updatedAt as JSON numbers (fractional epoch seconds), so the asserted wire shape for those members does not hold.
Why: an SDK deserializer bound to @timestampFormat("date-time") parses the member as a string; a number either throws a deserialization error or silently yields a nonsense instant. Any client that round-trips or logs the value, or compares it against another AWS timestamp, gets a wrong or failed parse rather than an instant.
Fix: the service's shared timestamp serializer (the response projection for DateTimestamp members), not the individual handlers.

Cause 2 of 4: GetKnowledgeBase (fixture bedrock-agent-delete-knowledge-base-conflict-recovery, step task#5).
AWS: Smithy: GetKnowledgeBase -> 200, output knowledgeBase: KnowledgeBase, whose createdAt/updatedAt are @required DateTimestamp @timestampFormat("date-time").
Emulator: 200 with $.knowledgeBase.createdAt and $.knowledgeBase.updatedAt as JSON numbers.
Why: same as cause 1, on the read path a client polls — so the mis-typed value is what most callers actually see.
Fix: same shared timestamp serializer.

Cause 3 of 4: CreateFlow (fixture bedrock-agent-flow-crud-lifecycle, step setup#2).
AWS: Smithy: CreateFlow -> 201; createdAt/updatedAt are @required DateTimestamp @timestampFormat("date-time") on the flow output.
Emulator: 201 with $.createdAt and $.updatedAt as JSON numbers.
Why: same as cause 1.
Fix: same shared timestamp serializer; the flow handlers inherit it.

Cause 4 of 4: GetFlow (fixture bedrock-agent-flow-crud-lifecycle, step task#3).
AWS: Smithy: GetFlow -> 200, output members createdAt/updatedAt @required DateTimestamp @timestampFormat("date-time").
Emulator: 200 with $.createdAt and $.updatedAt as JSON numbers.
Why: same as cause 1.
Fix: same shared timestamp serializer.

Note: the exact instants in the recordings are per-run values and are not a requirement; the requirement is the ISO-8601 string wire form of the member.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator:

Cause 1 — CreateKnowledgeBase, expected 202 / actual 202:
  $.knowledgeBase.createdAt: value_mismatch (expected '<shaped-not-asserted>' date-time string, got 1793293506.074)
  $.knowledgeBase.updatedAt: value_mismatch (expected '<shaped-not-asserted>' date-time string, got 1793293506.074)
  cluster spans 6 steps (bedrock-agent-data-source-custom-connector-lifecycle#setup#2, bedrock-agent-data-source-type-mismatch-errors#setup#2, bedrock-agent-delete-knowledge-base-conflict-recovery#setup#2, bedrock-agent-ingestion-job-lifecycle#setup#1, bedrock-agent-knowledge-base-documents-ingest-and-list#setup#2, bedrock-agent-knowledge-base-lifecycle#setup#2)

Cause 2 — GetKnowledgeBase, expected 200 / actual 200:
  $.knowledgeBase.createdAt: got 1793293750.698 (number, not a date-time string)
  $.knowledgeBase.updatedAt: got 1793293750.698
  cluster spans 2 steps (bedrock-agent-delete-knowledge-base-conflict-recovery#task#5, bedrock-agent-knowledge-base-lifecycle#task#3)

Cause 3 — CreateFlow, expected 201 / actual 201:
  $.createdAt: got 1793293813.547 ; $.updatedAt: got 1793293813.547
  cluster spans 2 steps (bedrock-agent-flow-crud-lifecycle#setup#2, bedrock-agent-flow-prepare-and-version#setup#2)

Cause 4 — GetFlow, expected 200 / actual 200:
  $.createdAt: got 1793293813.547 ; $.updatedAt: got 1793293813.547
  cluster spans 4 steps (bedrock-agent-flow-crud-lifecycle#task#3, bedrock-agent-flow-multi-isolation#task#4, bedrock-agent-flow-multi-isolation#task#5, bedrock-agent-flow-multi-isolation#task#7)

Smithy model: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json — KnowledgeBase.createdAt/updatedAt, DataSource.createdAt/updatedAt and GetFlow/UpdateFlow output createdAt/updatedAt all target DateTimestamp with timestampFormat "date-time".

## Steps to Reproduce
1. Replay fixture bedrock-agent-data-source-custom-connector-lifecycle, step setup#2 (CreateKnowledgeBase): inspect $.knowledgeBase.createdAt / $.knowledgeBase.updatedAt in the 202 body — they are JSON numbers, not ISO-8601 strings.
2. Replay bedrock-agent-delete-knowledge-base-conflict-recovery step task#5 (GetKnowledgeBase) and check the same two members.
3. Replay bedrock-agent-flow-crud-lifecycle step setup#2 (CreateFlow) and step task#3 (GetFlow) and check $.createdAt / $.updatedAt.
4. Every bedrock-agent response member targeting DateTimestamp must deserialize with an SDK configured for timestampFormat date-time.
