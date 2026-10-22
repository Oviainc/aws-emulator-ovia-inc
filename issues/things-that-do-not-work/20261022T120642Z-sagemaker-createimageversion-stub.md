Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-image-version-missing-base-image-rejected sagemaker.create-image-version-nonexistent-image-not-found-rejected
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.create-image-version-missing-base-image-rejected|run#7|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code} ({message!r})"
Issue-Key: sagemaker.create-image-version-nonexistent-image-not-found-rejected|run#8|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r} ({message!r})"
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker CreateImageVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImageVersion

## Description
`CreateImageVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_image_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateImageVersion` call, so each one unblocks only when `CreateImageVersion` works: `sagemaker.create-image-version-missing-base-image-rejected`, `sagemaker.create-image-version-nonexistent-image-not-found-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_image_version.rs`. Replaying the corpus, the emulator refused the `CreateImageVersion` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sagemaker_create_image_version_missing_base_image_rejected.py", line 39, in run
    assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code} ({message!r})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFound, got NotImplemented ('operation CreateImageVersion is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sagemaker emulator
Call CreateImageVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
