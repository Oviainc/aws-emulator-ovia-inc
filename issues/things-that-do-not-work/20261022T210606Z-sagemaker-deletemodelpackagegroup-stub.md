Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.delete-model-package-group-with-packages-conflict-rejected sagemaker.delete-model-package-group-with-versions-conflicts sagemaker.delete-model-package-group-with-versions-rejected sagemaker.model-package-group-create-describe-delete sagemaker.model-package-group-deletion-with-versions-rejected
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.delete-model-package-group-with-packages-conflict-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Key: sagemaker.delete-model-package-group-with-versions-conflicts|run#12|AssertionError|assert code == "ValidationException", (
Issue-Key: sagemaker.delete-model-package-group-with-versions-rejected|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}: {message!r}"
Issue-Key: sagemaker|DeleteModelPackageGroup|NotImplemented
Issue-Key: sagemaker.model-package-group-deletion-with-versions-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r} ({message!r})"
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# Implement sagemaker DeleteModelPackageGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteModelPackageGroup

## Description
`DeleteModelPackageGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_model_package_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `DeleteModelPackageGroup` call, so each one unblocks only when `DeleteModelPackageGroup` works: `sagemaker.delete-model-package-group-with-packages-conflict-rejected`, `sagemaker.delete-model-package-group-with-versions-conflicts`, `sagemaker.delete-model-package-group-with-versions-rejected`, `sagemaker.model-package-group-create-describe-delete`, `sagemaker.model-package-group-deletion-with-versions-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_model_package_group.rs`. Replaying the corpus, the emulator refused the `DeleteModelPackageGroup` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sagemaker_delete_model_package_group_with_packages_conflict_rejected.py", line 41, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation DeleteModelPackageGroup is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the sagemaker emulator
Call DeleteModelPackageGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
