Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.suppressed-destination-sandbox-refusal-effect-absent
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: sesv2.suppressed-destination-sandbox-refusal-effect-absent|run#8|AssertionError|assert pre.response['Error']['Code'] == 'NotFoundException', (
Issue-Commit: a40a343f88e572ec4b58650c779b2f3da5268f37

# Implement ses GetSuppressedDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a40a343f88e572ec4b58650c779b2f3da5268f37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSuppressedDestination

## Description
`GetSuppressedDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_suppressed_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetSuppressedDestination` call, so each one unblocks only when `GetSuppressedDestination` works: `ses.suppressed-destination-sandbox-refusal-effect-absent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_suppressed_destination.rs`. Replaying the corpus, the emulator refused the `GetSuppressedDestination` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ses_suppressed_destination_sandbox_refusal_effect_absent.py", line 85, in run
    assert pre.response['Error']['Code'] == 'NotFoundException', (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: setup precondition: 'sim2real-behav-supp@sim2real-behav-supp-e66c25663a5c.example.com' must not be suppressed yet, got {'Message': 'operation GetSuppressedDestination is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the ses emulator
Call GetSuppressedDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
