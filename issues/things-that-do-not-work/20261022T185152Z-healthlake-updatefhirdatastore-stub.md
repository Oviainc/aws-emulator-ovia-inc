Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.update-datastore-while-creating-rejected
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake.update-datastore-while-creating-rejected|run#12|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r} detail={detail!r}"
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# Implement healthlake UpdateFHIRDatastore: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFHIRDatastore

## Description
`UpdateFHIRDatastore` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-healthlake-healthlake/src/ops/update_fhir_datastore.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateFHIRDatastore` call, so each one unblocks only when `UpdateFHIRDatastore` works: `healthlake.update-datastore-while-creating-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-healthlake-healthlake/src/ops/update_fhir_datastore.rs`. Replaying the corpus, the emulator refused the `UpdateFHIRDatastore` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_healthlake_update_datastore_while_creating_rejected.py", line 48, in run
    assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r} detail={detail!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected AccessDeniedException, got 'NotImplemented' detail='operation UpdateFHIRDatastore is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the healthlake emulator
Call UpdateFHIRDatastore and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
