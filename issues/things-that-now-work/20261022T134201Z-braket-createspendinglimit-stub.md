Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: braket.braket-create-spending-limit-idempotency-under-concurrent-retries braket.braket-create-spending-limit-time-period-round-trips-exactly braket.braket-spending-limit-lifecycle-create-search-update-delete braket.braket-tag-and-untag-resource-lifecycle braket.braket-update-spending-limit-invalid-amount-pattern-rejected
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket.braket-create-spending-limit-idempotency-under-concurrent-retries|run#4|AssertionError|assert err["Code"] == "ValidationException", (
Issue-Key: braket|CreateSpendingLimit|NotImplemented
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Implement braket CreateSpendingLimit: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSpendingLimit

## Description
`CreateSpendingLimit` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-braket-braket/src/ops/create_spending_limit.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateSpendingLimit` call, so each one unblocks only when `CreateSpendingLimit` works: `braket.braket-create-spending-limit-idempotency-under-concurrent-retries`, `braket.braket-create-spending-limit-time-period-round-trips-exactly`, `braket.braket-spending-limit-lifecycle-create-search-update-delete`, `braket.braket-tag-and-untag-resource-lifecycle`, `braket.braket-update-spending-limit-invalid-amount-pattern-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-braket-braket/src/ops/create_spending_limit.rs`. Replaying the corpus, the emulator refused the `CreateSpendingLimit` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_braket_braket_create_spending_limit_idempotency_under_concurrent_retries.py", line 43, in run
    assert err["Code"] == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: simulator device: expected ValidationException, got 'NotImplemented': {'Message': 'operation CreateSpendingLimit is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the braket emulator
Call CreateSpendingLimit and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
