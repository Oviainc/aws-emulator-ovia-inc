Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.add-dataset-examples-concurrent-append-conflict bedrock-agentcore-control.dataset-lifecycle-and-versioning bedrock-agentcore-control.delete-dataset-examples-concurrent-delete-conflict
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.add-dataset-examples-concurrent-append-conflict|run#14|AssertionError|assert code == "ConflictException", f"undocumented AddDatasetExamples refusal: {code!r} ({message!r})"
Issue-Key: bedrock-agentcore-control|AddDatasetExamples|NotImplemented
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# Implement bedrock-agentcore-control AddDatasetExamples: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddDatasetExamples

## Description
`AddDatasetExamples` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/add_dataset_examples.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `AddDatasetExamples` call, so each one unblocks only when `AddDatasetExamples` works: `bedrock-agentcore-control.add-dataset-examples-concurrent-append-conflict`, `bedrock-agentcore-control.dataset-lifecycle-and-versioning`, `bedrock-agentcore-control.delete-dataset-examples-concurrent-delete-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/add_dataset_examples.rs`. Replaying the corpus, the emulator refused the `AddDatasetExamples` call: File "run.py", line 354, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_bedrock_agentcore_control_add_dataset_examples_concurrent_append_conflict.py", line 65, in run
    assert code == "ConflictException", f"undocumented AddDatasetExamples refusal: {code!r} ({message!r})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: undocumented AddDatasetExamples refusal: 'NotImplemented' ('operation AddDatasetExamples is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the bedrock-agentcore-control emulator
Call AddDatasetExamples and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
