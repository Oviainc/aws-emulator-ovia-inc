Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.flow-execution-events-ordering bedrock-agent-runtime.flow-execution-lifecycle-start-run-stop bedrock-agent-runtime.flow-execution-snapshot-availability-window bedrock-agent-runtime.flow-execution-terminal-state-immutable-rejected bedrock-agent-runtime.get-flow-execution-nonexistent-not-found bedrock-agent-runtime.stop-flow-execution-concurrent-race
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|StartFlowExecution|NotImplemented
Issue-Commit: 52f91172eb2f0209f53a5585da2848c5cfe26576

# Implement bedrock-agent-runtime StartFlowExecution: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `52f91172eb2f0209f53a5585da2848c5cfe26576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartFlowExecution

## Description
`StartFlowExecution` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/start_flow_execution.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `StartFlowExecution` call, so each one unblocks only when `StartFlowExecution` works: `bedrock-agent-runtime.flow-execution-events-ordering`, `bedrock-agent-runtime.flow-execution-lifecycle-start-run-stop`, `bedrock-agent-runtime.flow-execution-snapshot-availability-window`, `bedrock-agent-runtime.flow-execution-terminal-state-immutable-rejected`, `bedrock-agent-runtime.get-flow-execution-nonexistent-not-found`, `bedrock-agent-runtime.stop-flow-execution-concurrent-race`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/start_flow_execution.rs`. Replaying the corpus, the emulator refused the `StartFlowExecution` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartFlowExecution operation: operation StartFlowExecution is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-agent-runtime emulator
Call StartFlowExecution and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
