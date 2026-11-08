Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at|run#7|AssertionError|assert code == "ValidationException", (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# Implement bedrock-agentcore-control SynchronizeGatewayTargets: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SynchronizeGatewayTargets

## Description
`SynchronizeGatewayTargets` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/synchronize_gateway_targets.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `SynchronizeGatewayTargets` call, so each one unblocks only when `SynchronizeGatewayTargets` works: `bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/synchronize_gateway_targets.rs`. Replaying the corpus, the emulator refused the `SynchronizeGatewayTargets` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_bedrock_agentcore_control_synchronize_gateway_targets_advances_last_synchronized_at.py", line 94, in run
    assert code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected a PASSTHROUGH target's synchronize to be refused with ValidationException, got 'NotImplemented': 'operation SynchronizeGatewayTargets is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the bedrock-agentcore-control emulator
Call SynchronizeGatewayTargets and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
