Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.update-image-while-creating-rejected
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.update-image-while-creating-rejected|run#11|AssertionError|assert error.get("Code") == "ValidationException", (
Issue-Commit: 5b5a943942747caef372cc4ffaa7be31fd45b661

# Implement lambda-microvms UpdateMicrovmImage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b5a943942747caef372cc4ffaa7be31fd45b661`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMicrovmImage

## Description
`UpdateMicrovmImage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/update_microvm_image.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateMicrovmImage` call, so each one unblocks only when `UpdateMicrovmImage` works: `lambda-microvms.update-image-while-creating-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/update_microvm_image.rs`. Replaying the corpus, the emulator refused the `UpdateMicrovmImage` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_lambda_microvms_update_image_while_creating_rejected.py", line 50, in run
    assert error.get("Code") == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: UpdateMicrovmImage on a CREATING image answered {'Message': 'operation UpdateMicrovmImage is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}, expected ValidationException

## Steps to Reproduce
Boot the lambda-microvms emulator
Call UpdateMicrovmImage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
