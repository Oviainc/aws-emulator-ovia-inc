Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.delete-workload-identity-attached-to-runtime-blocked bedrock-agentcore-control.delete-workload-identity-nonexistent-returns-resource-not-found bedrock-agentcore-control.update-workload-identity-nonexistent-returns-resource-not-found bedrock-agentcore-control.workload-identity-lifecycle-transitions bedrock-agentcore-control.workload-identity-update-roundtrip
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateWorkloadIdentity|NotImplemented
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# Implement bedrock-agentcore-control CreateWorkloadIdentity: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkloadIdentity

## Description
`CreateWorkloadIdentity` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_workload_identity.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateWorkloadIdentity` call, so each one unblocks only when `CreateWorkloadIdentity` works: `bedrock-agentcore-control.delete-workload-identity-attached-to-runtime-blocked`, `bedrock-agentcore-control.delete-workload-identity-nonexistent-returns-resource-not-found`, `bedrock-agentcore-control.update-workload-identity-nonexistent-returns-resource-not-found`, `bedrock-agentcore-control.workload-identity-lifecycle-transitions`, `bedrock-agentcore-control.workload-identity-update-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_workload_identity.rs`. Replaying the corpus, the emulator refused the `CreateWorkloadIdentity` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateWorkloadIdentity operation: operation CreateWorkloadIdentity is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-agentcore-control emulator
Call CreateWorkloadIdentity and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
