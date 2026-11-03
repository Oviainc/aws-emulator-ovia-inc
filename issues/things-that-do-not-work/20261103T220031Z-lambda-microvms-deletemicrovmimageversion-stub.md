Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.delete-last-image-version-rejected lambda-microvms.delete-microvm-image-version-with-multiple-versions
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.delete-last-image-version-rejected|run#11|AssertionError|assert error.get("Code") == "ValidationException", (
Issue-Key: lambda-microvms|DeleteMicrovmImageVersion|NotImplemented
Issue-Commit: 2fe3407ad3a572d7c56bcafd231aa855d1c09ca1

# Implement lambda-microvms DeleteMicrovmImageVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2fe3407ad3a572d7c56bcafd231aa855d1c09ca1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMicrovmImageVersion

## Description
`DeleteMicrovmImageVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/delete_microvm_image_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteMicrovmImageVersion` call, so each one unblocks only when `DeleteMicrovmImageVersion` works: `lambda-microvms.delete-last-image-version-rejected`, `lambda-microvms.delete-microvm-image-version-with-multiple-versions`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/delete_microvm_image_version.rs`. Replaying the corpus, the emulator refused the `DeleteMicrovmImageVersion` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_lambda_microvms_delete_last_image_version_rejected.py", line 46, in run
    assert error.get("Code") == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: deleting the only version answered {'Message': 'operation DeleteMicrovmImageVersion is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}, expected ValidationException

## Steps to Reproduce
Boot the lambda-microvms emulator
Call DeleteMicrovmImageVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
