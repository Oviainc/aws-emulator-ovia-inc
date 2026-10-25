Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.lambda-sesv2-list-suppressed-destinations-missing-permission-negative
Issue-Key: sesv2|PutSuppressedDestination|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement sesv2 PutSuppressedDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSuppressedDestination

## Description
`PutSuppressedDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_suppressed_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutSuppressedDestination` call, so each one unblocks only when `PutSuppressedDestination` works: `lambda__sesv2.lambda-sesv2-list-suppressed-destinations-missing-permission-negative`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_suppressed_destination.rs`. Replaying the corpus, the emulator refused the `PutSuppressedDestination` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_lambda__sesv2_lambda_sesv2_list_suppressed_destinations_missing_permission_negative.py", line 50, in run
    assert err["Code"] == "BadRequestException", f"unexpected error seeding suppression entry: {err}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected error seeding suppression entry: {'Message': 'operation PutSuppressedDestination is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the sesv2 emulator
Call PutSuppressedDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
