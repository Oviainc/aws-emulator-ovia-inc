Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.concurrent-flow-alias-name-race bedrock-agent.create-flow-alias-conflicting-name-rejected bedrock-agent.e2e-flow-version-alias-promotion bedrock-agent.flow-alias-deploy-upgrade-and-rollback bedrock-agent.flow-delete-in-use-by-alias-rejected bedrock-agent.flow-invoke-end-to-end-via-runtime bedrock-agent.flow-lifecycle-draft-version-alias bedrock-agent.flow-version-delete-in-use-by-alias-rejected bedrock-agent.update-prepared-flow-leaves-existing-alias-unaffected
Issue-Key: bedrock-agent.concurrent-flow-alias-name-race|run#16|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}: {results}"
Issue-Key: bedrock-agent|CreateFlowAlias|NotImplemented
Issue-Commit: 0a8b28aea83ccc3190789fc779439d6e199d02f9

# Implement bedrock-agent CreateFlowAlias: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0a8b28aea83ccc3190789fc779439d6e199d02f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowAlias

## Description
`CreateFlowAlias` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_alias.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreateFlowAlias` call, so each one unblocks only when `CreateFlowAlias` works: `bedrock-agent.concurrent-flow-alias-name-race`, `bedrock-agent.create-flow-alias-conflicting-name-rejected`, `bedrock-agent.e2e-flow-version-alias-promotion`, `bedrock-agent.flow-alias-deploy-upgrade-and-rollback`, `bedrock-agent.flow-delete-in-use-by-alias-rejected`, `bedrock-agent.flow-invoke-end-to-end-via-runtime`, `bedrock-agent.flow-lifecycle-draft-version-alias`, `bedrock-agent.flow-version-delete-in-use-by-alias-rejected`, `bedrock-agent.update-prepared-flow-leaves-existing-alias-unaffected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_alias.rs`. Replaying the corpus, the emulator refused the `CreateFlowAlias` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_bedrock_agent_concurrent_flow_alias_name_race.py", line 86, in run
    assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}: {results}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected exactly 1 winner, got 0: [('NotImplemented', 'operation CreateFlowAlias is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation CreateFlowAlias is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation CreateFlowAlias is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation CreateFlowAlias is scaffolded but not implemented in this emulator'), ('NotImplemented', 'operation CreateFlowAlias is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the bedrock-agent emulator
Call CreateFlowAlias and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
