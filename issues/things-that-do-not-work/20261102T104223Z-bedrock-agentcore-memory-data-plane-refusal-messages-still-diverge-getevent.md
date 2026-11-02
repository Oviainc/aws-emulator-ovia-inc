Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.get-event-missing-resource-not-found bedrock-agentcore.memory-record-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.get-event-missing-resource-not-found|run#14|AssertionError|assert contrast_message == expected_contrast, (
Issue-Key: bedrock-agentcore.memory-record-lifecycle|run#17|AssertionError|assert message == "The memory record was not found", (
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# Memory data-plane refusal messages still diverge: GetEvent's pattern ValidationException echoes the value and anchors, and GetMemoryRecord/DeleteMemoryRecord not-found says "MemoryItem does not exist" (2 causes)

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMemoryRecord

## Description
Two net-new root causes in the bedrock-agentcore Memory data plane. In both the HTTP status and the error class already match AWS; the MESSAGE is the emulator's own text, so a client that handles these refusals by their documented wording stops matching.

Cause 1 of 2: the pattern-constraint ValidationException message for GetEvent's eventId echoes the rejected value and quotes the ANCHORED regex.
AWS: Smithy (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json): GetEvent GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}, eventId is a required @httpLabel with @pattern ^[0-9]+#[a-fA-F0-9]+$; ValidationException @httpError 400. Recorded AWS message form for a pattern miss on this member: the fixed text "1 validation error detected: Value at '" + the member path + "' failed to satisfy constraint: Member must satisfy regular expression pattern: " + the pattern with its ^ and $ anchors REMOVED. The only variable parts are the member path (here 'eventId') and the pattern; AWS does NOT quote the caller's offending value back for this member.
Emulator: returns 400 ValidationException with "1 validation error detected: Value '<the rejected eventId>' at 'eventId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+#[a-fA-F0-9]+$" — it inserts a "Value '<...>' " clause AWS omits and renders the pattern anchored. Both halves of the template are wrong; the specific rejected string in the replay output is a per-run request value and is not part of the requirement.
Why: equality/prefix matching on AWS's documented validation text fails, and the emulator reflects caller-supplied input into an error message AWS keeps value-free, which changes what gets logged.
Fix: the shared ValidationException message builder used by the Memory data-plane handlers' input validation — drop the value clause and strip the ^/$ anchors when rendering a @pattern. GetEvent's handler is services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/get_event.rs; the same builder feeds the other httpLabel pattern members of this service.
Note: the open issue 20261029T215145Z-bedrock-agentcore-memory-data-plane-refusal-fidelity-validationexception-messages-are.md describes this same builder defect as its Cause 1 and lists this check id, but it carries this check under a DIFFERENT failing assertion (its not-found assertion, which now passes); the check now fails later, on its contrast assertion, so that issue's replay key does not cover this failure and it is filed here. It must be replayed even if that issue is closed first.
Traces: bedrock-agentcore.get-event-missing-resource-not-found (the check's contrast step: GetEvent with a malformed eventId).

Cause 2 of 2: the ResourceNotFoundException message for a memory RECORD that does not exist is invented text that names the wrong thing.
AWS: Smithy: GetMemoryRecord GET /memories/{memoryId}/memoryRecord/{memoryRecordId} and DeleteMemoryRecord DELETE on the same resource, both modeling ResourceNotFoundException @httpError 404 with a single `message` member ("The exception that occurs when the specified resource does not exist. This can happen when using an invalid identifier or when trying to access a resource that has been deleted."). Recorded behavior only: for a well-formed memoryRecordId that does not exist in the container — including one a prior DeleteMemoryRecord removed — AWS answers 404 ResourceNotFoundException with the message "The memory record was not found". That sentence is entirely FIXED text: it has no variable part, and in particular it carries no record id, memory id or namespace.
Emulator: answers 404 ResourceNotFoundException with "MemoryItem does not exist" — right status and error class, a sentence AWS never produces, and it names an internal "MemoryItem" concept that is not the resource the API exposes. The same constant is served on both the read and the re-delete path, so a second DeleteMemoryRecord on an already-deleted id is refused with the same wrong wording.
Why: a client or SDK test that matches AWS's not-found sentence for a deleted record no longer matches, and the message leaks an emulator-internal resource name instead of the API's "memory record".
Fix: the ResourceNotFoundException message constant shared by the memory-record handlers — crate::state::MEMORY_RECORD_NOT_FOUND_MESSAGE in services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/state.rs, as used by src/ops/get_memory_record.rs (the None arm of the memory_record lookup) and src/ops/delete_memory_record.rs (the None arm of remove_memory_record).
Traces: bedrock-agentcore.memory-record-lifecycle (BatchCreateMemoryRecords, GetMemoryRecord, DeleteMemoryRecord, then GetMemoryRecord and a second DeleteMemoryRecord on the dead id).

## Evidence
Cause 1 — bedrock-agentcore.get-event-missing-resource-not-found [mismatch], at the contrast assertion (check line 58, `assert contrast_message == expected_contrast`):
  expected message "1 validation error detected: Value at 'eventId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]+#[a-fA-F0-9]+"
  got      message "1 validation error detected: Value 'totally not an event id!!' at 'eventId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+#[a-fA-F0-9]+$"
Status 400 and code ValidationException match on both sides. Fixed text of the template: "1 validation error detected: Value at '" ... "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member path ('eventId', from the request binding) and the unanchored pattern (from the model). The string 'totally not an event id!!' is this run's request value and is NOT part of the requirement — the defect is that any value appears at all, and that the pattern is printed with ^...$.

Cause 2 — bedrock-agentcore.memory-record-lifecycle [mismatch], at check line 69:
  AssertionError: expected message 'The memory record was not found', got 'MemoryItem does not exist'
The call is GetMemoryRecord on a memoryRecordId that BatchCreateMemoryRecords minted and DeleteMemoryRecord then removed; code was ResourceNotFoundException (404) on both sides, only the message diverges. The whole expected sentence is fixed text with no variable part. The identical constant is also what a second DeleteMemoryRecord on the same dead id returns, so both the read and the re-delete precondition report the wrong wording.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite (simparity-behavioral-aws-v1).
2. Cause 1: run bedrock-agentcore.get-event-missing-resource-not-found. After its not-found step, the check issues GetEvent with a malformed eventId ('totally not an event id!!') against a well-formed memoryId/actorId/sessionId and compares the ValidationException message: the emulator inserts a "Value '<input>' " clause and prints the pattern as ^[0-9]+#[a-fA-F0-9]+$ instead of [0-9]+#[a-fA-F0-9]+.
3. Cause 2: run bedrock-agentcore.memory-record-lifecycle. It creates a memory via bedrock-agentcore-control CreateMemory, writes one record with BatchCreateMemoryRecords, reads it back with GetMemoryRecord, deletes it with DeleteMemoryRecord, then calls GetMemoryRecord on the deleted memoryRecordId: the 404 ResourceNotFoundException message reads 'MemoryItem does not exist' instead of 'The memory record was not found'. Repeating DeleteMemoryRecord on the same id returns the same wrong message.
