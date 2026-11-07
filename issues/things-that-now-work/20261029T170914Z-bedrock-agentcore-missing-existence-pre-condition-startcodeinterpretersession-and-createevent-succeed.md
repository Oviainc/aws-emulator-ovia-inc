Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-code-interpreter-not-found-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-code-interpreter-not-found-errors bedrock-agentcore-create-json-event-not-found bedrock-agentcore-create-then-get-event-not-found
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|StartCodeInterpreterSession|4xx->2xx|$.message
Issue-Key: bedrock-agentcore|StartCodeInterpreterSession|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|StartCodeInterpreterSession|4xx->2xx|status
Issue-Key: bedrock-agentcore|CreateEvent|4xx->2xx|$.message
Issue-Key: bedrock-agentcore|CreateEvent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|CreateEvent|4xx->2xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Missing existence pre-condition: StartCodeInterpreterSession and CreateEvent succeed on nonexistent resources

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCodeInterpreterSession

## Description
Two net-new causes sharing one defect: two write operations never resolve the parent resource named in their request path, so instead of failing with ResourceNotFoundException they fabricate the resource and return success. The emulator is answering (the service is bootstrapped and other operations behave), so this is a missing validation step, not a missing operation.

Cause 1 of 2: StartCodeInterpreterSession accepts a code-interpreter identifier that does not exist.
AWS: Smithy: StartCodeInterpreterSession PUT /code-interpreters/{codeInterpreterIdentifier}/sessions/start, codeInterpreterIdentifier httpLabel required; modeled errors include ResourceNotFoundException @httpError 404 (shape doc: "The exception that occurs when the specified resource does not exist. This can happen when using an invalid identifier or when trying to access a resource that has been deleted."). Recorded response for an unknown identifier: HTTP 404, header x-amzn-errortype "ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/", body $.message "Code Interpreter not found: <codeInterpreterIdentifier>" — fixed text "Code Interpreter not found: ", variable part the codeInterpreterIdentifier httpLabel copied from the request URI.
Emulator: HTTP 200 with a StartCodeInterpreterSessionResponse — no $.message, no x-amzn-errortype — i.e. it mints a session against a code interpreter that was never registered.
Why: a client that typos or references a deleted code interpreter is told the session started and gets a sessionId; every subsequent Invoke/Stop call on that sessionId then behaves inconsistently, and error-handling paths that must surface "no such code interpreter" to the user are never exercised. A fail-fast 404 becomes a silent success.
Fix: StartCodeInterpreterSession's handler validation — resolve codeInterpreterIdentifier against the registered code interpreters (including the built-in aws.codeinterpreter.* identifiers) before allocating session state, and raise ResourceNotFoundException with the documented message when it misses.
Traces: bedrock-agentcore-code-interpreter-not-found-errors#task#3.

Cause 2 of 2: CreateEvent accepts a memoryId that does not exist.
AWS: Smithy: CreateEvent POST /memories/{memoryId}/events with @http code 201, memoryId httpLabel required @length min 12 @pattern ^(arn:...:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$; modeled errors include ResourceNotFoundException @httpError 404. Recorded response for an unknown memoryId: HTTP 404, x-amzn-errortype "ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/", $.message "Memory not found: <memoryId>" — fixed text "Memory not found: ", variable part the memoryId from the request path.
Emulator: HTTP 201 with a CreateEventOutput — the request is syntactically valid so the handler writes the event and implicitly conjures the parent memory, never checking that the memory was created.
Why: writes to a nonexistent (or deleted) memory are accepted and acknowledged with 201 and an eventId. Data appears durable to the client but is attached to a memory that does not exist on AWS, so an application built against the emulator never discovers that it must create the memory first; the same code fails at runtime on AWS. It also masks the delete-then-write case entirely.
Fix: CreateEvent's handler validation — resolve memoryId in the Memory data-plane state before persisting the event, and raise ResourceNotFoundException(404) with the documented message otherwise.
Traces: bedrock-agentcore-create-json-event-not-found#task#1 (the same cluster also covers bedrock-agentcore-create-then-get-event-not-found#task#1).

Note: once these two raise 404, their message text must follow AWS's fixed-label form; the sibling issue on ResourceNotFoundException message wording and resolution order covers the shared message builder. None of the open bedrock-agentcore issues covers these two pre-conditions.

## Evidence
Cause 1 StartCodeInterpreterSession (fixture bedrock-agentcore-code-interpreter-not-found-errors#task#3): expected status 404, got 200; $.message expected 'Code Interpreter not found: nonexistent.code.interpreter', got None; $headers.x-amzn-errortype expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/', got None.
Cause 2 CreateEvent (fixture bedrock-agentcore-create-json-event-not-found#task#1, cluster size 2): expected status 404, got 201; $.message expected 'Memory not found: sim2real-probe-fakememory-abcdefghij', got None; $headers.x-amzn-errortype expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/', got None.
In both, the identifier inside the expected message is the variable part taken from the request path; the fixed text is 'Code Interpreter not found: ' / 'Memory not found: ' and the error type header is invariant.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: fixture bedrock-agentcore-code-interpreter-not-found-errors, step task#3 — PUT /code-interpreters/nonexistent.code.interpreter/sessions/start. The emulator answers 200 with a sessionId; AWS answers 404 ResourceNotFoundException.
3. Cause 2: fixture bedrock-agentcore-create-json-event-not-found, step task#1 — POST /memories/<memoryId-never-created>/events with a valid payload. The emulator answers 201 with an event; AWS answers 404 ResourceNotFoundException. Same divergence at bedrock-agentcore-create-then-get-event-not-found#task#1.
