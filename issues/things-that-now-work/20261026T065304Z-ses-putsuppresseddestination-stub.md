Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.suppressed-destination-sandbox-refusal-effect-absent
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: sesv2.suppressed-destination-sandbox-refusal-effect-absent|run#13|AssertionError|assert code == 'BadRequestException', (
Issue-Commit: 3d58e892724a4dea7104d79d915d87cd1968e39f

# Implement ses PutSuppressedDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3d58e892724a4dea7104d79d915d87cd1968e39f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSuppressedDestination

## Description
`PutSuppressedDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_suppressed_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutSuppressedDestination` call, so each one unblocks only when `PutSuppressedDestination` works: `ses.suppressed-destination-sandbox-refusal-effect-absent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_suppressed_destination.rs`. Replaying the corpus, the emulator refused the `PutSuppressedDestination` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ses_suppressed_destination_sandbox_refusal_effect_absent.py", line 96, in run
    assert code == 'BadRequestException', (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: a sandboxed PutSuppressedDestination must be refused with BadRequestException, got 'NotImplemented': 'operation PutSuppressedDestination is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ses emulator
Call PutSuppressedDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
