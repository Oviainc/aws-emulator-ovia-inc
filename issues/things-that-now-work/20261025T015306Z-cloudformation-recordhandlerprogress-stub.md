Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.invalid-state-transition-record-handler-progress
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.invalid-state-transition-record-handler-progress|run#6|AssertionError|assert illegal_error.get("Code") == "ValidationError", (
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# Implement cloudformation RecordHandlerProgress: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecordHandlerProgress

## Description
`RecordHandlerProgress` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/record_handler_progress.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RecordHandlerProgress` call, so each one unblocks only when `RecordHandlerProgress` works: `cloudformation.invalid-state-transition-record-handler-progress`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/record_handler_progress.rs`. Replaying the corpus, the emulator refused the `RecordHandlerProgress` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudformation_invalid_state_transition_record_handler_progress.py", line 53, in run
    assert illegal_error.get("Code") == "ValidationError", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: RecordHandlerProgress reporting an illegal SUCCESS->IN_PROGRESS transition against a BearerToken that was never issued: expected ValidationError, got {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation RecordHandlerProgress is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the cloudformation emulator
Call RecordHandlerProgress and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
