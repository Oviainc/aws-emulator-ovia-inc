Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.create-registry-duplicate-name-conflict bedrock-agentcore-control.registry-record-lifecycle-with-approval bedrock-agentcore-control.submit-registry-record-for-approval-concurrent-submission-conflict
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateRegistry|NotImplemented
Issue-Key: bedrock-agentcore-control.submit-registry-record-for-approval-concurrent-submission-conflict|run#6|AssertionError|assert create_code == "AccessDeniedException", (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# Implement bedrock-agentcore-control CreateRegistry: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRegistry

## Description
`CreateRegistry` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_registry.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateRegistry` call, so each one unblocks only when `CreateRegistry` works: `bedrock-agentcore-control.create-registry-duplicate-name-conflict`, `bedrock-agentcore-control.registry-record-lifecycle-with-approval`, `bedrock-agentcore-control.submit-registry-record-for-approval-concurrent-submission-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_registry.rs`. Replaying the corpus, the emulator refused the `CreateRegistry` call: File "check_bedrock_agentcore_control_create_registry_duplicate_name_conflict.py", line 43, in run
    _assert_denied_naming_op(lambda: client.create_registry(name=name), "CreateRegistry")
  File "check_bedrock_agentcore_control_create_registry_duplicate_name_conflict.py", line 62, in _assert_denied_naming_op
    assert code == "AccessDeniedException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected AccessDeniedException for CreateRegistry, got 'NotImplemented': 'operation CreateRegistry is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the bedrock-agentcore-control emulator
Call CreateRegistry and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
