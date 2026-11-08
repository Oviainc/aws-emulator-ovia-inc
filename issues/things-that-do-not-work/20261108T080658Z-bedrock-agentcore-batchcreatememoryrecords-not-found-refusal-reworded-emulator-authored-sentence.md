Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-memory-record-crud-not-found-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-memory-record-crud-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|BatchCreateMemoryRecords|4xx->4xx|$.message
Issue-Commit: 824a021fbc81c5b46e0dca2fae74f8c18911ab8d

# BatchCreateMemoryRecords not-found refusal reworded: emulator-authored sentence instead of AWS's fixed MemoryItem message

> _Note: this issue was identified against the code base at commit `824a021fbc81c5b46e0dca2fae74f8c18911ab8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateMemoryRecords

## Description
Cause 1 of 1: BatchCreateMemoryRecords now refuses a write into a non-existent memory container with the right status and error class, but with a message this emulator authored instead of the sentence AWS produces.

AWS: Smithy (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json) models BatchCreateMemoryRecords as POST /memories/{memoryId}/memoryRecords/batchCreate with @http code 201, memoryId a @required @httpLabel (MemoryId, @length min 12, @pattern admitting either the bare id or the memory ARN), and ResourceNotFoundException with @httpError 404 among its errors. Recorded behavior: for a well-formed memoryId that names no existing container the call is refused 404 ResourceNotFoundException with the message "MemoryItem does not exist". That message is ENTIRELY fixed text — it has no variable part at all: it names neither the memoryId from the path nor any record identifier from the request.

Emulator: answers the correct 404 ResourceNotFoundException but with the message "Memory not found: <memoryId>" — the shared helper crate::state::resource_not_found_message (src/state.rs:1876-1878, `format!("{} not found: {identifier}", resource.label())` with NotFoundResource::Memory's label "Memory" at src/state.rs:1860), invoked from the existence gate at src/ops/batch_create_memory_records.rs:192-204. So the lead-in is this emulator's own generic template and the memory id from the request path is interpolated into a message AWS never parameterizes.

Why: a client or SDK conformance test that matches AWS's fixed "MemoryItem does not exist" sentence to recognize a missing memory container stops recognizing it, and the interpolated memory id changes what gets logged on this path. Because AWS's sentence is wholly fixed, any substring or equality match a caller writes against it fails here, so the refusal cannot be classified by message as it can against AWS.

Fix: the not-found arm of this operation's handler (src/ops/batch_create_memory_records.rs, the `if !held` branch): it must emit AWS's fixed sentence for this operation rather than routing through the generic resource_not_found_message helper, whose "<Label> not found: <identifier>" template is correct for the other readers that DO name their resource (e.g. GetEvent's "Memory not found: {memoryId}") but is not AWS's wording here. Keep the 404 and the ResourceNotFoundException class, which already match.

Related but not covering: the open issue 20261102T113600Z-bedrock-agentcore-agentcore-not-found-refusals-diverge-wrong-status-wrong.md recorded (its Cause 4) that this operation had NO existence pre-condition and answered 201 for a missing container. That half has landed — the gate exists and the status/error class are now right — and its replay key no longer matches this step (the divergence is now 404->404 on $.message, not 4xx->2xx). This remaining wording divergence is net-new, is not owned by any open issue's replay key, and must be replayed on its own.

Traces: bedrock-agentcore-memory-record-crud-not-found-errors#task#1.

## Evidence
From the wire replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false).

Fixture bedrock-agentcore-memory-record-crud-not-found-errors, step task#1, BatchCreateMemoryRecords [mismatch], status 404 expected and 404 actual (status and x-amzn-errortype now agree):
  $.message: expected 'MemoryItem does not exist', got 'Memory not found: sim2real-probe-fakememory-abcdefghij'

Fixed vs variable text: the expected message is 100% fixed text ('MemoryItem does not exist') with NO variable part. In the actual message, 'Memory not found: ' is the emulator's own fixed lead-in (NotFoundResource::Memory's label plus the shared ' not found: ' template) and the trailing token is the memoryId taken from the request path — the id shown above is this run's request value, not part of any requirement.

Source of the divergence (not a request-specific value): services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/batch_create_memory_records.rs:192-204 (the `if !held` refusal) calling crate::state::resource_not_found_message at src/state.rs:1876-1878 with NotFoundResource::Memory (label at src/state.rs:1860).

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus simparity-wire-aws-v1.
2. POST /memories/<well-formed memoryId that no memory container exists for>/memoryRecords/batchCreate with one record (fixture bedrock-agentcore-memory-record-crud-not-found-errors, step task#1).
3. The response is 404 ResourceNotFoundException, as AWS answers, but its message reads 'Memory not found: <the memoryId from the path>' instead of AWS's fixed 'MemoryItem does not exist'.
4. Equivalent replay: re-run fixture bedrock-agentcore-memory-record-crud-not-found-errors, step task#1.
