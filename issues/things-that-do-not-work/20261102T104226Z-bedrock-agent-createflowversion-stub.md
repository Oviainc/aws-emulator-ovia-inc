Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.concurrent-flow-alias-name-race bedrock-agent.create-flow-alias-conflicting-name-rejected bedrock-agent.e2e-flow-version-alias-promotion bedrock-agent.flow-alias-dangling-version-reference-rejected bedrock-agent.flow-alias-deploy-upgrade-and-rollback bedrock-agent.flow-definition-representation-round-trip bedrock-agent.flow-delete-in-use-by-alias-rejected bedrock-agent.flow-invoke-end-to-end-via-runtime bedrock-agent.flow-lifecycle-draft-version-alias bedrock-agent.flow-version-delete-in-use-by-alias-rejected bedrock-agent.flow-version-monotonic-numbering bedrock-agent.flow-version-snapshots-definition-immutably bedrock-agent.update-prepared-flow-leaves-existing-alias-unaffected bedrock-agent.validate-flow-definition-catches-cycle-before-version
Issue-Key: bedrock-agent|CreateFlowVersion|NotImplemented
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# Implement bedrock-agent CreateFlowVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowVersion

## Description
`CreateFlowVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `CreateFlowVersion` call, so each one unblocks only when `CreateFlowVersion` works: `bedrock-agent.concurrent-flow-alias-name-race`, `bedrock-agent.create-flow-alias-conflicting-name-rejected`, `bedrock-agent.e2e-flow-version-alias-promotion`, `bedrock-agent.flow-alias-dangling-version-reference-rejected`, `bedrock-agent.flow-alias-deploy-upgrade-and-rollback`, `bedrock-agent.flow-definition-representation-round-trip`, `bedrock-agent.flow-delete-in-use-by-alias-rejected`, `bedrock-agent.flow-invoke-end-to-end-via-runtime`, `bedrock-agent.flow-lifecycle-draft-version-alias`, `bedrock-agent.flow-version-delete-in-use-by-alias-rejected`, `bedrock-agent.flow-version-monotonic-numbering`, `bedrock-agent.flow-version-snapshots-definition-immutably`, `bedrock-agent.update-prepared-flow-leaves-existing-alias-unaffected`, `bedrock-agent.validate-flow-definition-catches-cycle-before-version`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs`. Replaying the corpus, the emulator refused the `CreateFlowVersion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateFlowVersion operation: operation CreateFlowVersion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-agent emulator
Call CreateFlowVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
