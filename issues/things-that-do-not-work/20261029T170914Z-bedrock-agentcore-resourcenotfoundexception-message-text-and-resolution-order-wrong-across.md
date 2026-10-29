Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-browse-actors-and-sessions-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-browse-actors-and-sessions-not-found bedrock-agentcore-code-interpreter-not-found-errors bedrock-agentcore-create-then-get-event-not-found bedrock-agentcore-list-then-delete-event-not-found
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|ListSessions|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|GetCodeInterpreterSession|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|GetEvent|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|ListEvents|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|DeleteEvent|4xx->4xx|$.message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# ResourceNotFoundException message text and resolution order wrong across memory and code-interpreter lookups

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSessions

## Description
Five net-new causes, all one defect family: when a bedrock-agentcore lookup misses, the emulator returns the right HTTP status (404) and the right error type, but it composes its OWN message text instead of AWS's, and in the Memory data-plane operations it also reports the WRONG resource because it validates the innermost identifier before it has resolved the outer one. AWS's ResourceNotFoundException message for this service is a fixed-label form: the capitalised resource label, then the literal ": ", then the identifier that failed to resolve ("Memory not found: <memoryId>", "Session not found: <sessionId>"). The fixed text is the label and the ": " separator; the only variable part is the identifier copied from the request. The emulator instead emits lowercase, relative-clause prose ("memory <id> not found", "session <id> not found for actor <id> in memory <id>"), which is never AWS's wording for any of these operations.

Cause 1 of 5: ListSessions on a memory that does not exist.
AWS: Smithy: ListSessions POST /memories/{memoryId}/actor/{actorId}/sessions, error ResourceNotFoundException @httpError 404, member message: NonBlankString; shape doc: "The exception that occurs when the specified resource does not exist." Recorded response: 404 with x-amzn-errortype ResourceNotFoundException and message "Memory not found: <memoryId>" — fixed text "Memory not found: ", variable part the memoryId httpLabel from the request.
Emulator: 404 with message "memory <memoryId> not found" — correct status and type, wrong wording (lowercase label, no ": " separator, trailing "not found").
Why: clients and SDK test suites that match AWS's documented message, and any human reading the error, see a message AWS never produces; message-matching error handling for a missing memory silently stops matching.
Fix: the shared ResourceNotFoundException message builder used by the Memory data-plane handlers' not-found path.
Traces: bedrock-agentcore-browse-actors-and-sessions-not-found#task#2.

Cause 2 of 5: GetCodeInterpreterSession on a session id that does not exist.
AWS: Smithy: GetCodeInterpreterSession GET /code-interpreters/{codeInterpreterIdentifier}/sessions/get, sessionId httpQuery required @pattern ^[0-9a-zA-Z]{1,40}$, error ResourceNotFoundException @httpError 404. Recorded response: 404, message "Session not found: <sessionId>" — fixed text "Session not found: ", variable part the sessionId query value.
Emulator: 404 with message "session <sessionId> not found for code interpreter <codeInterpreterIdentifier>" — it appends the code-interpreter identifier, which AWS's message for this error does not carry at all.
Why: a client cannot rely on the message shape; the extra clause leaks an identifier AWS does not disclose in this error and breaks any equality or prefix match on the documented text.
Fix: the GetCodeInterpreterSession handler's not-found path / the code-interpreter error message builder.
Traces: bedrock-agentcore-code-interpreter-not-found-errors#task#1.

Cause 3 of 5: GetEvent against a memory that does not exist reports the EVENT as missing instead of the memory.
AWS: Smithy: GetEvent GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}, all four httpLabels required, error ResourceNotFoundException @httpError 404. Recorded response: 404, message "Memory not found: <memoryId>" — AWS resolves the path outside-in, so a nonexistent memoryId fails on the MEMORY and the message names the memory, not the event.
Emulator: 404, message "event <eventId> not found for actor <actorId> in session <sessionId> of memory <memoryId>" — it skipped memory resolution and reported the innermost identifier as the missing resource.
Why: the client is told the event is absent from an existing memory when in fact the memory does not exist, so retry/create-memory recovery logic takes the wrong branch; the wording is also not AWS's.
Fix: GetEvent's handler must resolve memory -> actor -> session -> event in that order and raise on the first miss, using the shared message builder.
Traces: bedrock-agentcore-create-then-get-event-not-found#task#2.

Cause 4 of 5: ListEvents against a memory that does not exist reports the SESSION as missing instead of the memory.
AWS: Smithy: ListEvents POST /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}, error ResourceNotFoundException @httpError 404. Recorded response: 404, message "Memory not found: <memoryId>" — same outside-in resolution order as GetEvent.
Emulator: 404, message "session <sessionId> not found for actor <actorId> in memory <memoryId>" — reports the session, naming a memory it has just failed to find.
Why: same mis-attribution as cause 3: the caller concludes the memory exists and only the session is missing.
Fix: ListEvents' handler resolution order plus the shared message builder.
Traces: bedrock-agentcore-list-then-delete-event-not-found#task#1.

Cause 5 of 5: DeleteEvent on a memory that does not exist.
AWS: Smithy: DeleteEvent DELETE /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}, error ResourceNotFoundException @httpError 404. Recorded response: 404, message "Memory not found: <memoryId>".
Emulator: 404, message "memory <memoryId> not found" — right resource, wrong wording (same lowercase form as cause 1).
Why: as cause 1 — message-based handling of a missing memory on delete no longer matches AWS.
Fix: the shared ResourceNotFoundException message builder on DeleteEvent's not-found path.
Traces: bedrock-agentcore-list-then-delete-event-not-found#task#2.

None of the four currently open bedrock-agentcore issues (InvokeAgentRuntime stub, CreateAgentRuntime S3 pre-condition, gateway MCP Lambda target, ActiveSessionCount metric) covers error-message text or not-found resolution order, so this is net-new.

## Evidence
Cause 1 ListSessions: expected 404 $.message 'Memory not found: sim2real-probe-fakememory-abcdefghij', got 404 'memory sim2real-probe-fakememory-abcdefghij not found' (fixed text differs; the id is the request's memoryId and is only the template's variable part).
Cause 2 GetCodeInterpreterSession: expected 404 'Session not found: <sessionId>', got 'session <sessionId> not found for code interpreter aws.codeinterpreter.v1'.
Cause 3 GetEvent: expected 404 'Memory not found: <memoryId>', got 'event <eventId> not found for actor <actorId> in session <sessionId> of memory <memoryId>'.
Cause 4 ListEvents: expected 404 'Memory not found: <memoryId>', got 'session <sessionId> not found for actor <actorId> in memory <memoryId>'.
Cause 5 DeleteEvent: expected 404 'Memory not found: <memoryId>', got 'memory <memoryId> not found'.
In all five the HTTP status (404) and error type (ResourceNotFoundException) already match; only the message payload — and in causes 3 and 4 which resource is named — diverge.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: fixture bedrock-agentcore-browse-actors-and-sessions-not-found, step task#2 — ListSessions on a memoryId that was never created; compare $.message.
3. Cause 2: fixture bedrock-agentcore-code-interpreter-not-found-errors, step task#1 — GetCodeInterpreterSession with a sessionId that does not exist.
4. Cause 3: fixture bedrock-agentcore-create-then-get-event-not-found, step task#2 — GetEvent under a memoryId that does not exist.
5. Cause 4: fixture bedrock-agentcore-list-then-delete-event-not-found, step task#1 — ListEvents under a memoryId that does not exist.
6. Cause 5: fixture bedrock-agentcore-list-then-delete-event-not-found, step task#2 — DeleteEvent under a memoryId that does not exist.
Each step returns 404 but with a message AWS does not produce; causes 3 and 4 additionally name the wrong resource.
