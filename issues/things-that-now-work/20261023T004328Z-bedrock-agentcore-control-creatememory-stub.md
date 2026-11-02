Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.agentcore-memory-record-delete-removes-only-that-record
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agentcore-control|CreateMemory|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Implement bedrock-agentcore-control CreateMemory: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMemory

## Description
`CreateMemory` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_memory.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateMemory` call, so each one unblocks only when `CreateMemory` works: `bedrock-agent-runtime.agentcore-memory-record-delete-removes-only-that-record`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_memory.rs`. Replaying the corpus, the emulator refused the `CreateMemory` call: File "check_bedrock_agent_runtime_agentcore_memory_record_delete_removes_only_that_record.py", line 25, in run
    memory_id = _create_memory(ctx, control)
  File "check_bedrock_agent_runtime_agentcore_memory_record_delete_removes_only_that_record.py", line 109, in _create_memory
    raise AssertionError(
        f"CreateMemory (bedrock-agentcore-control) failed during setup: {exc!r}"
    ) from exc
AssertionError: CreateMemory (bedrock-agentcore-control) failed during setup: ClientError('An error occurred (NotImplemented) when calling the CreateMemory operation: operation CreateMemory is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the bedrock-agentcore-control emulator
Call CreateMemory and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
