Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: savingsplans.delete-queued-savings-plan-not-found-rejected savingsplans.delete-queued-savings-plan-transitions-to-queued-deleted savingsplans.return-savings-plan-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans.delete-queued-savings-plan-not-found-rejected|run#7|AssertionError|assert err["Code"] == "ResourceNotFoundException", f"expected ResourceNotFoundException for a foreign id, got {err!r}"
Issue-Key: savingsplans|DeleteQueuedSavingsPlan|NotImplemented
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# Implement savingsplans DeleteQueuedSavingsPlan: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteQueuedSavingsPlan

## Description
`DeleteQueuedSavingsPlan` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/delete_queued_savings_plan.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteQueuedSavingsPlan` call, so each one unblocks only when `DeleteQueuedSavingsPlan` works: `savingsplans.delete-queued-savings-plan-not-found-rejected`, `savingsplans.delete-queued-savings-plan-transitions-to-queued-deleted`, `savingsplans.return-savings-plan-wrong-state-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/delete_queued_savings_plan.rs`. Replaying the corpus, the emulator refused the `DeleteQueuedSavingsPlan` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_savingsplans_delete_queued_savings_plan_not_found_rejected.py", line 30, in run
    assert err["Code"] == "ResourceNotFoundException", f"expected ResourceNotFoundException for a foreign id, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException for a foreign id, got {'Message': 'operation DeleteQueuedSavingsPlan is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the savingsplans emulator
Call DeleteQueuedSavingsPlan and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
