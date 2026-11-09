Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.delete-memory-record-invalid-input-malformed-id bedrock-agentcore.get-event-malformed-id-invalid-input bedrock-agentcore.create-event-malformed-payload-rejected bedrock-agentcore.get-event-missing-resource-not-found bedrock-agentcore.agc-memory-event-session-isolation bedrock-agentcore.metadata-pattern-violation-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.delete-memory-record-invalid-input-malformed-id|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: bedrock-agentcore.get-event-malformed-id-invalid-input|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: bedrock-agentcore.create-event-malformed-payload-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: bedrock-agentcore.get-event-missing-resource-not-found|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: bedrock-agentcore.agc-memory-event-session-isolation|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: bedrock-agentcore.metadata-pattern-violation-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# Memory data-plane refusal fidelity: ValidationException messages are emulator-composed, GetEvent not-found names the wrong resource and uses invented wording, and CreateEvent accepts metadata that violates its modeled pattern

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEvent

## Description
Five net-new causes in the bedrock-agentcore Memory data plane. Four are refusal-message fidelity defects (the status and error class are right, the message AWS produces is not), and the fifth is an over-permissive accept: a constraint the model states is not enforced at all.

Cause 1 of 5: pattern-constraint ValidationException messages echo the rejected value and quote the ANCHORED regex.
AWS: Smithy: DeleteMemoryRecord.memoryRecordId @pattern ^mem-[a-zA-Z0-9-_]*$ @length 40-50 (httpLabel); GetEvent.eventId @pattern ^[0-9]+#[a-fA-F0-9]+$ (httpLabel); both operations model ValidationException @httpError 400. Recorded AWS message form for a pattern miss: fixed text '1 validation error detected: Value at ' + the quoted member path + ' failed to satisfy constraint: Member must satisfy regular expression pattern: ' + the pattern WITHOUT its ^/$ anchors. The variable parts are only the member path and the pattern; AWS does not quote the offending value back for these members.
Emulator: emits '1 validation error detected: Value '<the rejected value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: ^<pattern>$' — it inserts the value and prints the anchored pattern. Both halves of the message are wrong.
Why: a client or SDK test that matches AWS's documented validation text stops matching, and the emulator reflects caller-supplied input back into an error message AWS keeps value-free, which changes what gets logged.
Fix: the shared ValidationException message builder used by the Memory data-plane handlers' input validation (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/delete_memory_record.rs, src/ops/get_event.rs and the common validation module) — drop the value clause and strip the anchors when rendering the pattern.
Traces: bedrock-agentcore.delete-memory-record-invalid-input-malformed-id, bedrock-agentcore.get-event-malformed-id-invalid-input.

Cause 2 of 5: enum-constraint ValidationException message echoes the rejected value and lists the enum values in the wrong order.
AWS: Smithy: CreateEvent.payload is PayloadTypeList; the conversational member's role targets Role, an enum whose modeled values are ASSISTANT, USER, TOOL, OTHER; ValidationException @httpError 400. Recorded AWS message: fixed text '1 validation error detected: Value at ' + member path + ' failed to satisfy constraint: Member must satisfy enum value set: [USER, OTHER, TOOL, ASSISTANT]' — i.e. no value echo, and the value set rendered in that order (recorded behavior only; it is not the model's declaration order).
Emulator: '1 validation error detected: Value '<rejected role>' at 'payload.1.member.conversational.role' failed to satisfy constraint: Member must satisfy enum value set: [ASSISTANT, USER, TOOL, OTHER]' — value echoed, set rendered in model order.
Why: same as cause 1 — equality/prefix matching on AWS's validation text fails, and a client that parses the allowed-value list out of the message sees a different ordering than AWS emits.
Fix: the enum branch of the same shared ValidationException message builder (CreateEvent's payload validation, src/ops/create_event.rs).
Traces: bedrock-agentcore.create-event-malformed-payload-rejected.

Cause 3 of 5: GetEvent under a memory that does not exist reports the EVENT as missing instead of the MEMORY.
AWS: Smithy: GetEvent GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}, all four identifiers required httpLabels, ResourceNotFoundException @httpError 404 with a single message member. AWS resolves the path outside-in, so a nonexistent memoryId fails on the memory and the message is the fixed-label form 'Memory not found: ' + the request's memoryId.
Emulator: 'event <eventId> not found for actor <actorId> in session <sessionId> of memory <memoryId>' — it skips memory resolution and names the innermost identifier as the missing resource.
Why: the caller is told the event is absent from an existing memory when the memory itself does not exist, so create-memory/retry recovery logic takes the wrong branch; the wording is also not AWS's.
Fix: GetEvent's handler resolution order (memory -> actor -> session -> event, raising on the first miss) plus the shared ResourceNotFoundException message builder (src/ops/get_event.rs).
Note: the open issue 20261029T170914Z-bedrock-agentcore-resourcenotfoundexception-message-text-and-resolution-order-wrong-across.md describes this same defect from the wire corpus (its Cause 3). It carries no behavioral check id, so the check below is filed here and must be replayed even if that issue is closed first.
Traces: bedrock-agentcore.get-event-missing-resource-not-found.

Cause 4 of 5: GetEvent for an event that exists in a SIBLING session uses invented wording for the cross-session miss.
AWS: Smithy: GetEvent ResourceNotFoundException @httpError 404. Recorded AWS message when the memory, actor and session all exist but the eventId belongs to another session of the same actor: fixed text 'Unable to find event with id ' + the request's eventId — the eventId is the only variable part, and no actor/session/memory identifier appears.
Emulator: 'event <eventId> not found for actor <actorId> in session <sessionId> of memory <memoryId>' — the isolation decision itself is correct (404 ResourceNotFoundException), but the message is the emulator's own prose and discloses three identifiers AWS does not put in this error.
Why: a client cannot distinguish the cross-session case from the missing-memory case by message, which is how AWS's two forms differ, and message-based handling of session isolation stops matching.
Fix: GetEvent's handler cross-session not-found path plus the shared message builder (src/ops/get_event.rs).
Traces: bedrock-agentcore.agc-memory-event-session-isolation.

Cause 5 of 5: CreateEvent ACCEPTS a metadata stringValue that violates its modeled pattern.
AWS: Smithy: CreateEvent.metadata is MetadataMap (@length 0-15); MetadataValue is a union whose stringValue member is @length 0-256 @pattern ^[a-zA-Z0-9\s._:/=+@-]*$; ValidationException @httpError 400. A value containing '#' is outside the allowlist, so AWS refuses the call with '1 validation error detected: Value at ' + the member path (metadata.<key>.member.stringValue) + ' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9\s._:/=+@-]*' (same value-free, unanchored form as cause 1) and creates nothing.
Emulator: the CreateEvent call SUCCEEDS — no error is raised for a stringValue of the form 'web#chat'. The metadata pattern is never evaluated.
Why: this is an over-permissive accept, the worst class here: the emulator stores an event AWS would reject, so a client's bad metadata is only discovered in production, and every negative test for metadata validation passes vacuously against the emulator.
Fix: CreateEvent's input validation (src/ops/create_event.rs) — enforce MetadataMap key and MetadataValue.stringValue length+pattern before any state mutation, raising ValidationException via the shared message builder.
Traces: bedrock-agentcore.metadata-pattern-violation-rejected.

## Evidence
Cause 1, DeleteMemoryRecord: expected message "1 validation error detected: Value at 'memoryRecordId' failed to satisfy constraint: Member must satisfy regular expression pattern: mem-[a-zA-Z0-9-_]*", got "1 validation error detected: Value 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' at 'memoryRecordId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^mem-[a-zA-Z0-9-_]*$". GetEvent: expected "1 validation error detected: Value at 'eventId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]+#[a-fA-F0-9]+", got "1 validation error detected: Value 'not-a-valid-event-id' at 'eventId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+#[a-fA-F0-9]+$". The quoted value and the ^...$ anchors are the divergence; the rejected value itself is per-run and is NOT part of the requirement.
Cause 2, CreateEvent: expected "1 validation error detected: Value at 'payload.1.member.conversational.role' failed to satisfy constraint: Member must satisfy enum value set: [USER, OTHER, TOOL, ASSISTANT]", got "1 validation error detected: Value 'NOT_A_VALID_ROLE' at 'payload.1.member.conversational.role' failed to satisfy constraint: Member must satisfy enum value set: [ASSISTANT, USER, TOOL, OTHER]".
Cause 3, GetEvent on a nonexistent memory: expected ResourceNotFoundException message 'Memory not found: <memoryId>' (fixed text 'Memory not found: ' + the request's memoryId), got 'event <eventId> not found for actor <actorId> in session <sessionId> of memory <memoryId>'.
Cause 4, GetEvent across sessions: expected ResourceNotFoundException message 'Unable to find event with id <eventId>' (fixed text 'Unable to find event with id ' + the request's eventId), got 'event <eventId> not found for actor <actorId> in session <sessionId> of memory <memoryId>'. Code was ResourceNotFoundException on both sides.
Cause 5, CreateEvent with metadata={'channel': {'stringValue': 'web#chat'}}: expected ValidationException with "1 validation error detected: Value at 'metadata.channel.member.stringValue' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9\\s._:/=+@-]*" and no event created; got a successful CreateEvent (AssertionError: expected the call to be rejected, but it SUCCEEDED).

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite (simparity-behavioral-aws-v1).
2. Cause 1: run bedrock-agentcore.delete-memory-record-invalid-input-malformed-id (DeleteMemoryRecord with a memoryRecordId that fails ^mem-[a-zA-Z0-9-_]*$) and bedrock-agentcore.get-event-malformed-id-invalid-input (GetEvent with an eventId that fails ^[0-9]+#[a-fA-F0-9]+$); compare the ValidationException message.
3. Cause 2: run bedrock-agentcore.create-event-malformed-payload-rejected — CreateEvent with payload[1].conversational.role set to a non-enum string; compare the ValidationException message.
4. Cause 3: run bedrock-agentcore.get-event-missing-resource-not-found — GetEvent with a well-formed but nonexistent memoryId/actorId/sessionId/eventId; the emulator names the event, AWS names the memory.
5. Cause 4: run bedrock-agentcore.agc-memory-event-session-isolation — create a memory, write one event in session A and one in session B for the same actor, then GetEvent with session B's sessionId and session A's eventId; compare the ResourceNotFoundException message.
6. Cause 5: run bedrock-agentcore.metadata-pattern-violation-rejected — CreateEvent into an existing memory/session with metadata={'channel': {'stringValue': 'web#chat'}}; the emulator accepts it, AWS rejects it with ValidationException and creates nothing.
