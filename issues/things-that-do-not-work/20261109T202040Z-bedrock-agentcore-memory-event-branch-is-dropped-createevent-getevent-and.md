Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-memory-event-written-and-read-back
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-memory-event-written-and-read-back
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|CreateEvent|2xx->2xx|$.event.branch
Issue-Key: bedrock-agentcore|ListEvents|2xx->2xx|$.events[*].branch
Issue-Key: bedrock-agentcore|GetEvent|2xx->2xx|$.event.branch
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# Memory event branch is dropped: CreateEvent, GetEvent and ListEvents never return Event.branch (3 causes)

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEvent

## Description
The AgentCore Memory data plane silently discards the branch a client attaches to an event: the write echoes no branch and every read of the same event omits it, so the member round-trips as absent on all three operations. The three causes below are one dropped member observed on the write and on both read paths; a fixer must close all three.

Cause 1 of 3 — CreateEvent does not echo the created event's branch (fixture bedrock-agentcore-memory-event-written-and-read-back, step task#4).
AWS: Smithy (com.amazonaws.bedrockagentcore) — CreateEvent input member `branch` targets the `Branch` structure (members `name`, required, @length 1..100, @pattern ^[a-zA-Z0-9][a-zA-Z0-9-_]*$; optional `rootEventId`); CreateEvent output member `event` is required and targets `Event`, whose member `branch` targets the same `Branch` structure and is documented "The branch information for the event." HTTP 201 POST /memories/{memoryId}/events. AWS therefore returns the created event carrying the branch the request supplied (the request named a branch, so the response's `event.branch` is that structure, e.g. `{"name": "<the branch name from the request>"}`).
Emulator: answers 201 as expected, but `$.event.branch` is absent (expected the Branch structure the request supplied, got None).
Why: a client that writes an event into a named conversation branch gets a 201 whose event claims no branch. Code that reads the branch back off the create response to continue a thread (or to build the next event's `branch.rootEventId`) sees an unbranched event and either falls back to the default trunk or fails its own assertion, even though the write was accepted.
Fix: the CreateEvent handler/state — the request's `branch` must be stored on the event record, and the Event serializer must emit `branch` whenever the stored event has one.

Cause 2 of 3 — ListEvents omits branch on every listed event (fixture bedrock-agentcore-memory-event-written-and-read-back, step task#5).
AWS: Smithy — ListEvents (POST /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}, 200) output member `events` is a required `EventList` of the same `Event` shape, so each listed event carries the `branch` it was created with.
Emulator: 200 with the event present, but `$.events[0].branch` is missing (expected the Branch structure of the event as written, got None).
Why: branch is how AgentCore Memory separates conversation threads inside one actor/session. With it absent from the listing, a client cannot tell which thread any event belongs to and cannot group or filter a session's events by branch.
Fix: the ListEvents projection of the stored event — the same Event serializer must include `branch`.

Cause 3 of 3 — GetEvent omits branch on the single-event read (fixture bedrock-agentcore-memory-event-written-and-read-back, step task#6).
AWS: Smithy — GetEvent (GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}, 200) output member `event` is required and targets `Event`, including `branch`.
Emulator: 200, but `$.event.branch` is missing (expected the Branch structure of the event as written, got None).
Why: the authoritative read of an event loses its thread membership, so a client cannot reconstruct a branched conversation from stored events, and `branch.rootEventId` chaining is impossible.
Fix: the GetEvent handler's Event serialization/state read — carry the persisted `branch` through.

No open issue covers this: the open memory data-plane issue 20261102T104223Z-bedrock-agentcore-memory-data-plane-refusal-messages-still-diverge-getevent concerns refusal message wording, not the Event representation.

## Evidence
Fixture bedrock-agentcore-memory-event-written-and-read-back writes one event with a branch and reads it back three ways.

step task#4 CreateEvent — expected 201, got 201; $.event.branch: missing (expected {'name': 'main'}, got None).
step task#5 ListEvents — expected 200, got 200; $.events[0].branch: missing (expected {'name': 'main'}, got None).
step task#6 GetEvent — expected 200, got 200; $.event.branch: missing (expected {'name': 'main'}, got None).

The branch name in the recording ('main') is the value that recording's request supplied; the requirement is that whatever Branch the request carried is persisted and re-emitted, not that name. Statuses match in all three steps — only the member is dropped.

## Steps to Reproduce
1. Create an AgentCore Memory resource.
2. CreateEvent POST /memories/{memoryId}/events with required memoryId/actorId/eventTimestamp/payload and a `branch` of {"name": "<branchName>"}.
3. Observe the 201 response: `event.branch` is absent instead of the submitted Branch.
4. ListEvents POST /memories/{memoryId}/actor/{actorId}/sessions/{sessionId} — `events[0].branch` is absent.
5. GetEvent GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId} — `event.branch` is absent.
Replay: fixture bedrock-agentcore-memory-event-written-and-read-back, steps task#4, task#5, task#6.
