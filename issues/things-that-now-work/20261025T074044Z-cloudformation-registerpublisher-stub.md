Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.publisher-registration-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|RegisterPublisher|NotImplemented
Issue-Commit: b4d018646e260fb276f96a884ffb19d3cc9227fb

# Implement cloudformation RegisterPublisher: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b4d018646e260fb276f96a884ffb19d3cc9227fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterPublisher

## Description
`RegisterPublisher` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/register_publisher.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RegisterPublisher` call, so each one unblocks only when `RegisterPublisher` works: `cloudformation.publisher-registration-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/register_publisher.rs`. Replaying the corpus, the emulator refused the `RegisterPublisher` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudformation_publisher_registration_lifecycle.py", line 36, in run
    assert error["Code"] == "CFNRegistryException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected CFNRegistryException, got NotImplemented: operation RegisterPublisher is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudformation emulator
Call RegisterPublisher and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
