Issue-Source: trace-sweep-divergence
Issue-Trace: acc-memory-update-and-strategy-round-trip
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-memory-update-and-strategy-round-trip bedrock-agentcore-control-memory-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetMemory|2xx->2xx|$.memory.description
Issue-Key: bedrock-agentcore-control|GetMemory|2xx->2xx|$.memory.eventExpiryDuration
Issue-Key: bedrock-agentcore-control|GetMemory|2xx->2xx|$.memory.extractionHealth
Issue-Key: bedrock-agentcore-control|GetMemory|2xx->2xx|$.memory.strategies[*]
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Memory reads ignore UpdateMemory and drop extractionHealth and strategies

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMemory

## Description
The emulator's memory resource does not reflect mutations and does not serialize the health/strategy parts of the Memory structure: GetMemory keeps returning the pre-update description and expiry, never lists a created strategy, and never emits extractionHealth. 3 causes must all be fixed before this issue is closed.

Cause 1 of 3: GetMemory returns pre-update state after UpdateMemory, and omits extractionHealth.
AWS: UpdateMemory (Smithy: PUT /memories/{memoryId}/update, HTTP 202; input members `description` -> Description @length 1..4096 and `eventExpiryDuration` -> Integer @range 3..365) applies the change to the memory, and GetMemory (GET /memories/{memoryId}/details, HTTP 200, output `memory` -> Memory, required) then returns the updated values — `memory.description` as supplied by the update and `memory.eventExpiryDuration` as supplied by the update (Memory.eventExpiryDuration is required). The response additionally carries a memory-level extraction-health object reporting the failed extraction-job count; that member is not in the on-disk Memory shape, so recorded behavior only: AWS returns `memory.extractionHealth` with `failedMemoryExtractionJobCount` 0 on a healthy memory.
Emulator: GetMemory answers 200 but with the values the memory had before the update — the description from create and the original eventExpiryDuration — and with `memory.extractionHealth` absent.
Why: UpdateMemory appears to succeed (202) while the resource never changes, so a client that updates a memory's retention window or description and re-reads it sees its write silently lost; code that polls extractionHealth to decide whether extraction is healthy finds the field missing and cannot make the decision at all.
Fix: the UpdateMemory handler's state mutation (it must persist description/eventExpiryDuration onto the stored memory) and the Memory serializer shared by GetMemory, which must populate extractionHealth.
Traces: acc-memory-update-and-strategy-round-trip#task#6.

Cause 2 of 3: GetMemory never lists a created memory strategy.
AWS: Memory.strategies (Smithy: Memory.strategies -> MemoryStrategyList) lists the memory's strategies, each a MemoryStrategy with required `strategyId`, `name`, `type`, `namespaces` and `namespaceTemplates` plus `description`, `status`, `createdAt`/`updatedAt`; after a strategy is added to a memory, GetMemory returns it in that list, with its own extraction-health object (recorded behavior only, as above) and status ACTIVE once provisioned.
Emulator: GetMemory returns no `strategies` entry at all — the created SEMANTIC strategy is absent from the memory read — alongside the same stale description/eventExpiryDuration and missing memory-level extractionHealth from cause 1.
Why: the strategy round-trip is unobservable: a client that creates a strategy and then reads the memory to learn its generated strategyId, namespaces or status gets an empty list, so it cannot address the strategy it just created.
Fix: the memory state's strategy collection and the Memory serializer used by GetMemory, so added strategies are stored against the memory and rendered in `strategies`.
Traces: acc-memory-update-and-strategy-round-trip#task#9.

Cause 3 of 3: extractionHealth is missing on a plain memory read, with no update involved.
AWS: on a freshly created memory, GetMemory's `memory` object carries the extraction-health object with `failedMemoryExtractionJobCount` 0 (recorded behavior only — not present in the on-disk Memory shape).
Emulator: GetMemory returns 200 with `memory.extractionHealth` absent on a memory that was only created, confirming the omission is in the Memory serializer and not a side effect of the update path.
Why: every consumer of memory health sees the field missing on every memory, so health can never be asserted even on the happy path.
Fix: the Memory serializer shared by the memory read handlers, which must always emit extractionHealth.
Traces: bedrock-agentcore-control-memory-lifecycle#task#2.

## Evidence
Cause 1 — acc-memory-update-and-strategy-round-trip#task#6, GetMemory, expected 200 / actual 200: $.memory.description expected the post-update description, got the pre-update one ('acc memory after update' vs 'acc memory before update' in the recorded run); $.memory.eventExpiryDuration expected the updated value, got the original (60 vs 30); $.memory.extractionHealth missing (expected {'failedMemoryExtractionJobCount': 0}, got None).
Cause 2 — acc-memory-update-and-strategy-round-trip#task#9, GetMemory, 200 vs 200: same three divergences plus $.memory.strategies[0] missing — the expected entry is a MemoryStrategy with type SEMANTIC, status ACTIVE, its namespaces/namespaceTemplates, its own extractionHealth {'failedMemoryExtractionJobCount': 0}, and shaped strategyId/createdAt/updatedAt; the emulator returned None.
Cause 3 — bedrock-agentcore-control-memory-lifecycle#task#2, GetMemory, 200 vs 200: $.memory.extractionHealth missing (expected {'failedMemoryExtractionJobCount': 0}, got None).
The literal descriptions, durations, strategy name and generated ids above are the recorded run's values; the requirements are that an applied update is readable back, that an added strategy appears in `strategies`, and that extractionHealth is present.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture acc-memory-update-and-strategy-round-trip, step task#6 — CreateMemory, UpdateMemory changing description and eventExpiryDuration, then GetMemory; compare $.memory.description, $.memory.eventExpiryDuration and $.memory.extractionHealth.
3. Cause 2: same fixture, step task#9 — after adding a SEMANTIC memory strategy, GetMemory and inspect $.memory.strategies.
4. Cause 3: fixture bedrock-agentcore-control-memory-lifecycle, step task#2 — GetMemory on a newly created memory and check for $.memory.extractionHealth.
