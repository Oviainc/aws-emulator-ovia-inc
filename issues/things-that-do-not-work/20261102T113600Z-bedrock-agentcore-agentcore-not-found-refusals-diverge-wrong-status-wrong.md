Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-agentcore-traj-invoke-deleted-runtime
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-agentcore-traj-invoke-deleted-runtime bedrock-agentcore-code-interpreter-not-found-errors bedrock-agentcore-create-then-get-event-not-found bedrock-agentcore-memory-record-crud-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|InvokeAgentRuntime|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|InvokeAgentRuntime|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|InvokeAgentRuntime|4xx->4xx|status
Issue-Key: bedrock-agentcore|StopCodeInterpreterSession|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|GetEvent|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|BatchCreateMemoryRecords|4xx->2xx|$.message
Issue-Key: bedrock-agentcore|BatchCreateMemoryRecords|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|BatchCreateMemoryRecords|4xx->2xx|status
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# AgentCore not-found refusals diverge: wrong status, wrong resource and reworded ResourceNotFoundException messages (4 causes)

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateMemoryRecords

## Description
FOUR net-new root causes in the bedrock-agentcore data plane, all on the resource-existence / not-found refusal path: the emulator either never checks that the addressed container exists, checks it in the wrong order, or answers the right status and error class with its own invented sentence. A fixer must close all FOUR before submitting.

Cause 1 of 4: InvokeAgentRuntime enforces the runtimeSessionId header length BEFORE resolving the runtime/endpoint, so a deleted runtime can never report not-found.
AWS: Smithy (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json): InvokeAgentRuntime is POST /runtimes/{agentRuntimeArn}/invocations with @http code 200; agentRuntimeArn is a @required @httpLabel, qualifier an @httpQuery, and runtimeSessionId an @httpHeader X-Amzn-Bedrock-AgentCore-Runtime-Session-Id with @idempotencyToken and @length min 33 max 256; modeled errors include ResourceNotFoundException @httpError 404 and ValidationException @httpError 400. Recorded behavior: invoking an agentRuntimeArn whose runtime has been deleted answers 404 ResourceNotFoundException whose message template is the fixed text "No endpoint or agent found with qualifier '" + the effective qualifier + "' for agent '" + the agentRuntimeArn + "'"; the only variable parts are the qualifier (DEFAULT when the request sends no ?qualifier) and the runtime ARN taken from the request path. The recorded request ALSO carried an under-length runtimeSessionId and AWS still answered the 404, so on this path target resolution precedes enforcement of that header's @length. Hypothesis: AWS resolves the runtime/endpoint first and only validates the session header once the target resolves.
Emulator: answers 400 with x-amzn-errortype ValidationException and the framework's constraint template "1 validation error detected: Value '<the header value>' at 'runtimeSessionId' failed to satisfy constraint: Member must have length greater than or equal to 33" (the particular header value in the replay is this run's request value, not part of any requirement). The refusal is produced by the centralized deserialize/validate step, so the handler is never reached; the handler itself is additionally still a scaffold stub (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/invoke_agent_runtime.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and answers 501). The open issue 20261023T113357Z-bedrock-agentcore-invokeagentruntime-stub.md covers only the unimplemented handler and does not carry this replay key; it does not cover the ORDERING defect, which would keep masking the recorded 404 even after the handler is written.
Why: a client that deletes a runtime and then invokes it is told its own session header is malformed instead of that the agent/endpoint is gone, so delete-then-invoke reconciliation loops retry a request that can never succeed and never surface ResourceNotFoundException.
Fix: InvokeAgentRuntime's handler plus the ordering between the operation's input validation and its target resolution (src/ops/invoke_agent_runtime.rs in services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore): resolve agentRuntimeArn + qualifier against runtime state and raise ResourceNotFoundException with the template above before the runtimeSessionId @length check is applied.
Traces: bedrock-agentcore-agentcore-traj-invoke-deleted-runtime#task#4.

Cause 2 of 4: StopCodeInterpreterSession's not-found message is emulator-authored and names the interpreter instead of the session.
AWS: Smithy: StopCodeInterpreterSession is PUT /code-interpreters/{codeInterpreterIdentifier}/sessions/stop with a @required @httpQuery sessionId (@pattern ^[0-9a-zA-Z]{1,40}$) and models ResourceNotFoundException @httpError 404. Recorded behavior only: stopping a well-formed sessionId that does not exist answers 404 ResourceNotFoundException with the message "Sandbox session not found: " + the sessionId from the request — that lead-in is fixed text and the session id is the only variable part; the message does NOT name the code-interpreter identifier.
Emulator: answers 404 with the right error class but the message "session <sessionId> not found for code interpreter <codeInterpreterIdentifier>" — a sentence AWS never produces, which reorders the template and appends the interpreter id. (The open stub issue 20261029T215145Z-bedrock-agentcore-stopcodeinterpretersession-stub.md predates the implementation: the operation is now served, so that issue does not cover this wording defect.)
Why: a client or SDK test that matches AWS's "Sandbox session not found: <id>" sentence to distinguish a vanished session from an unknown interpreter stops matching, and the leaked interpreter identifier changes what gets logged.
Fix: the not-found arm of StopCodeInterpreterSession's handler and the shared message constant it uses (src/ops/stop_code_interpreter_session.rs and the session-lookup message helper in src/state.rs).
Traces: bedrock-agentcore-code-interpreter-not-found-errors#task#2.

Cause 3 of 4: GetEvent reports the EVENT as missing when it is the MEMORY container that does not exist.
AWS: Smithy: GetEvent is GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}, all four @required @httpLabel members, and models ResourceNotFoundException @httpError 404. Recorded behavior only: when memoryId is well-formed (it satisfies the MemoryId @pattern) but names no existing memory, AWS resolves the container FIRST and answers 404 ResourceNotFoundException with "Memory not found: " + the memoryId from the path — fixed lead-in plus one variable part, the memory id. The event id is never mentioned, because the event was never looked up.
Emulator: answers 404 ResourceNotFoundException with "Event not found: " + the eventId from the path, i.e. it skips the memory-existence precondition entirely and reports the innermost resource. Two open issues describe the same family but neither carries this replay key: 20261029T215145Z-bedrock-agentcore-memory-data-plane-refusal-fidelity-validationexception-messages-are.md (its GetEvent cause is described against a different failing assertion) and 20261102T104223Z-bedrock-agentcore-memory-data-plane-refusal-messages-still-diverge-getevent.md (that one is about GetEvent's @pattern ValidationException text and GetMemoryRecord's not-found constant). This step must be replayed even if both are closed first.
Why: a client cannot tell "this memory container is gone/mistyped" from "this event id is unknown inside a live container" — the two drive different recovery (re-create or re-resolve the memory, versus re-list events), and the emulator reports the second for the first.
Fix: the resource-resolution order in GetEvent's handler (src/ops/get_event.rs): look up the memory container and raise "Memory not found: {memoryId}" before the event lookup; the same precondition belongs on the sibling event/record readers that address a memory by path.
Traces: bedrock-agentcore-create-then-get-event-not-found#task#2.

Cause 4 of 4: BatchCreateMemoryRecords has NO memory-existence precondition — it creates records in a memory that does not exist and answers 201.
AWS: Smithy: BatchCreateMemoryRecords is POST /memories/{memoryId}/memoryRecords/batchCreate with @http code 201, memoryId a @required @httpLabel, and models ResourceNotFoundException @httpError 404. Recorded behavior only: for a well-formed memoryId that names no existing memory the call is refused with 404 ResourceNotFoundException, header x-amzn-errortype ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/, and the message "MemoryItem does not exist" — that sentence is entirely fixed text with no variable part (it carries no memory id and no record id).
Emulator: answers the success code 201 with successfulRecords/failedRecords, minting records against a container that was never created; no error is raised at all.
Why: the failure is silent and writes are accepted into nowhere: a client that mistypes or races a DeleteMemory believes its records were stored, and every later read of them (GetMemoryRecord/ListMemoryRecords) is empty or 404 with no refusal to explain it. It also removes the only signal that a memory needs creating. The open issue 20261029T170914Z-bedrock-agentcore-missing-existence-pre-condition-startcodeinterpretersession-and-createevent-succeed.md reports the same missing-precondition fault for StartCodeInterpreterSession and CreateEvent only; it does not name BatchCreateMemoryRecords and does not carry this replay key.
Why this is one issue with the three above: all four are the same existence/precedence gate in this service's data plane.
Fix: the memory-resolution gate at the top of BatchCreateMemoryRecords' handler (src/ops/batch_create_memory_records.rs): resolve memoryId in crate::state before any record is minted and raise ResourceNotFoundException with the fixed message when it is absent.
Traces: bedrock-agentcore-memory-record-crud-not-found-errors#task#1.

## Evidence
All four from the wire replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false).

Cause 1 — fixture bedrock-agentcore-agentcore-traj-invoke-deleted-runtime, step task#4, InvokeAgentRuntime [mismatch]:
  status: expected 404, got 400
  $.message: expected "No endpoint or agent found with qualifier 'DEFAULT' for agent 'arn:aws:bedrock-agentcore:<region>:<account>:runtime/<runtime-id>'", got "1 validation error detected: Value 'short' at 'runtimeSessionId' failed to satisfy constraint: Member must have length greater than or equal to 33"
  $headers.x-amzn-errortype: got 'ValidationException' (expected side shaped-not-asserted; 404 on this operation is ResourceNotFoundException)
Fixed text of the expected message: "No endpoint or agent found with qualifier '" / "' for agent '" / "'". Variable parts: the qualifier (DEFAULT, absent ?qualifier) and the agentRuntimeArn from the request path. The ARN and the header value 'short' shown above are this run's request values, not requirements.

Cause 2 — fixture bedrock-agentcore-code-interpreter-not-found-errors, step task#2, StopCodeInterpreterSession [mismatch], status 404 on both sides:
  $.message: expected 'Sandbox session not found: <sessionId>', got 'session <sessionId> not found for code interpreter aws.codeinterpreter.v1'
Fixed text: 'Sandbox session not found: '. Variable part: the sessionId from the ?sessionId query. The interpreter identifier the emulator appends is not part of AWS's template.

Cause 3 — fixture bedrock-agentcore-create-then-get-event-not-found, step task#2, GetEvent [mismatch], status 404 on both sides:
  $.message: expected 'Memory not found: <memoryId>', got 'Event not found: <eventId>'
Fixed text: 'Memory not found: '. Variable part: the memoryId from the path. The emulator names the event instead, proving the container was never resolved.

Cause 4 — fixture bedrock-agentcore-memory-record-crud-not-found-errors, step task#1, BatchCreateMemoryRecords [mismatch]:
  status: expected 404, got 201
  $.message: expected 'MemoryItem does not exist', got None (no error body at all)
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agentcore.common/', got None
The expected message is wholly fixed text with no variable part.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus simparity-wire-aws-v1.
2. Cause 1 — bedrock-agentcore-agentcore-traj-invoke-deleted-runtime, task#4: create an agent runtime, delete it, then POST /runtimes/{deletedRuntimeArn}/invocations with no ?qualifier and an under-length X-Amzn-Bedrock-AgentCore-Runtime-Session-Id. The emulator answers 400 ValidationException about runtimeSessionId; AWS answers 404 ResourceNotFoundException "No endpoint or agent found with qualifier 'DEFAULT' for agent '<arn>'".
3. Cause 2 — bedrock-agentcore-code-interpreter-not-found-errors, task#2: PUT /code-interpreters/aws.codeinterpreter.v1/sessions/stop?sessionId=<well-formed id that was never started>. Status 404 and ResourceNotFoundException are right; the message reads 'session <id> not found for code interpreter aws.codeinterpreter.v1' instead of 'Sandbox session not found: <id>'.
4. Cause 3 — bedrock-agentcore-create-then-get-event-not-found, task#2: GET /memories/<well-formed memoryId that does not exist>/actor/<actorId>/sessions/<sessionId>/events/<well-formed eventId>. The 404 message reads 'Event not found: <eventId>' instead of 'Memory not found: <memoryId>'.
5. Cause 4 — bedrock-agentcore-memory-record-crud-not-found-errors, task#1: POST /memories/<well-formed memoryId that does not exist>/memoryRecords/batchCreate with one record. The emulator returns 201 with successfulRecords; AWS refuses with 404 ResourceNotFoundException 'MemoryItem does not exist'.
