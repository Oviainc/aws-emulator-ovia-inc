Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iottwinmaker.batch-put-property-values-partial-entry-errors
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker.batch-put-property-values-partial-entry-errors|run#12|AssertionError|assert real_ref_error["Code"] == "ValidationException", f"unexpected error={real_ref_error!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement iottwinmaker BatchPutPropertyValues: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutPropertyValues

## Description
`BatchPutPropertyValues` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/batch_put_property_values.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `BatchPutPropertyValues` call, so each one unblocks only when `BatchPutPropertyValues` works: `iottwinmaker.batch-put-property-values-partial-entry-errors`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/batch_put_property_values.rs`. Replaying the corpus, the emulator refused the `BatchPutPropertyValues` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_iottwinmaker_batch_put_property_values_partial_entry_errors.py", line 68, in run
    assert real_ref_error["Code"] == "ValidationException", f"unexpected error={real_ref_error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected error={'Message': 'operation BatchPutPropertyValues is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the iottwinmaker emulator
Call BatchPutPropertyValues and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
