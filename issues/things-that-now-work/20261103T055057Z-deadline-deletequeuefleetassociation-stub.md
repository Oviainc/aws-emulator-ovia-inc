Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.queue-fleet-association-delete-before-stopped-rejected
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.queue-fleet-association-delete-before-stopped-rejected|run#12|AssertionError|assert error.get("Code") == "ConflictException", f"expected ConflictException, got {error}"
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# Implement deadline DeleteQueueFleetAssociation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteQueueFleetAssociation

## Description
`DeleteQueueFleetAssociation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-deadline-deadline/src/ops/delete_queue_fleet_association.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteQueueFleetAssociation` call, so each one unblocks only when `DeleteQueueFleetAssociation` works: `deadline.queue-fleet-association-delete-before-stopped-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-deadline-deadline/src/ops/delete_queue_fleet_association.rs`. Replaying the corpus, the emulator refused the `DeleteQueueFleetAssociation` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_deadline_queue_fleet_association_delete_before_stopped_rejected.py", line 58, in run
    assert error.get("Code") == "ConflictException", f"expected ConflictException, got {error}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ConflictException, got {'Message': 'operation DeleteQueueFleetAssociation is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the deadline emulator
Call DeleteQueueFleetAssociation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
