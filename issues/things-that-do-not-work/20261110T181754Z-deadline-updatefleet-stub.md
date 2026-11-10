Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.create-fleet-idempotent-client-token-concurrent
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline|UpdateFleet|NotImplemented
Issue-Commit: 122d2e23ed306dec374f1768a21e001304c75e9d

# Implement deadline UpdateFleet: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `122d2e23ed306dec374f1768a21e001304c75e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFleet

## Description
`UpdateFleet` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-deadline-deadline/src/ops/update_fleet.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateFleet` call, so each one unblocks only when `UpdateFleet` works: `deadline.create-fleet-idempotent-client-token-concurrent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-deadline-deadline/src/ops/update_fleet.rs`. Replaying the corpus, the emulator refused the `UpdateFleet` call: File "run.py", line 365, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_deadline_create_fleet_idempotent_client_token_concurrent.py", line 102, in run
    assert error.get("Code") == "ResourceNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException, got {'Message': 'operation UpdateFleet is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the deadline emulator
Call UpdateFleet and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
