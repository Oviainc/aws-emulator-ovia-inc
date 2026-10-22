Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityagent.add-artifact-get-round-trip securityagent.add-artifact-invalid-type-rejected securityagent.agent-space-lifecycle-crud securityagent.agent-space-update-persists-fields securityagent.batch-get-agent-spaces-partial-not-found securityagent.batch-get-artifact-metadata-round-trip securityagent.code-review-job-reaches-terminal-state securityagent.list-artifacts-reflects-additions-and-deletions
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|CreateAgentSpace|NotImplemented
Issue-Commit: 00577afa9aa0eead33f7dafc8adac2dd3659e1fc

# Implement securityagent CreateAgentSpace: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `00577afa9aa0eead33f7dafc8adac2dd3659e1fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentSpace

## Description
`CreateAgentSpace` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-securityagent-securityagent/src/ops/create_agent_space.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `CreateAgentSpace` call, so each one unblocks only when `CreateAgentSpace` works: `securityagent.add-artifact-get-round-trip`, `securityagent.add-artifact-invalid-type-rejected`, `securityagent.agent-space-lifecycle-crud`, `securityagent.agent-space-update-persists-fields`, `securityagent.batch-get-agent-spaces-partial-not-found`, `securityagent.batch-get-artifact-metadata-round-trip`, `securityagent.code-review-job-reaches-terminal-state`, `securityagent.list-artifacts-reflects-additions-and-deletions`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-securityagent-securityagent/src/ops/create_agent_space.rs`. Replaying the corpus, the emulator refused the `CreateAgentSpace` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAgentSpace operation: operation CreateAgentSpace is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the securityagent emulator
Call CreateAgentSpace and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
