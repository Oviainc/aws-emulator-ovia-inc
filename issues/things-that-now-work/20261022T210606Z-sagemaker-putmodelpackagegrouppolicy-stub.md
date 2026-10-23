Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.concurrent-put-model-package-group-policy-last-writer-wins sagemaker.delete-model-package-group-policy-removes-policy sagemaker.delete-model-package-group-policy-then-get-not-found sagemaker.e2e-model-package-group-policy-controls-access-lifecycle sagemaker.e2e-model-package-group-policy-full-flow sagemaker.model-package-group-policy-byte-roundtrip sagemaker.model-package-group-policy-concurrent-put-last-writer-wins sagemaker.model-package-group-policy-document-json-byte-equality sagemaker.model-package-group-policy-embedded-blob-preserved sagemaker.model-package-group-policy-json-round-trip sagemaker.model-package-group-policy-malformed-document-rejected sagemaker.model-package-group-policy-put-get-delete-consistency sagemaker.model-package-group-policy-put-get-delete-flow sagemaker.model-package-group-policy-put-get-delete-round-trip sagemaker.model-package-group-policy-round-trip-byte-equality sagemaker.model-package-group-policy-unicode-byte-round-trip sagemaker.model-package-group-policy-unicode-round-trip sagemaker.put-model-package-group-policy-concurrent-last-writer-wins sagemaker.put-model-package-group-policy-concurrent-overwrite sagemaker.put-model-package-group-policy-concurrent-overwrite-deterministic sagemaker.put-model-package-group-policy-concurrent-overwrite-last-writer-wins sagemaker.put-model-package-group-policy-concurrent-writes-serialize-to-one-policy sagemaker.put-model-package-group-policy-invalid-json-conflict
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.concurrent-put-model-package-group-policy-last-writer-wins|run#12|AssertionError|assert len(errors) < 2, f"both concurrent PutModelPackageGroupPolicy calls failed: {errors}"
Issue-Key: sagemaker|PutModelPackageGroupPolicy|NotImplemented
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# Implement sagemaker PutModelPackageGroupPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutModelPackageGroupPolicy

## Description
`PutModelPackageGroupPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/put_model_package_group_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

23 recorded unit(s) fail on their `PutModelPackageGroupPolicy` call, so each one unblocks only when `PutModelPackageGroupPolicy` works: `sagemaker.concurrent-put-model-package-group-policy-last-writer-wins`, `sagemaker.delete-model-package-group-policy-removes-policy`, `sagemaker.delete-model-package-group-policy-then-get-not-found`, `sagemaker.e2e-model-package-group-policy-controls-access-lifecycle`, `sagemaker.e2e-model-package-group-policy-full-flow`, `sagemaker.model-package-group-policy-byte-roundtrip`, `sagemaker.model-package-group-policy-concurrent-put-last-writer-wins`, `sagemaker.model-package-group-policy-document-json-byte-equality`, `sagemaker.model-package-group-policy-embedded-blob-preserved`, `sagemaker.model-package-group-policy-json-round-trip`, `sagemaker.model-package-group-policy-malformed-document-rejected`, `sagemaker.model-package-group-policy-put-get-delete-consistency`, `sagemaker.model-package-group-policy-put-get-delete-flow`, `sagemaker.model-package-group-policy-put-get-delete-round-trip`, `sagemaker.model-package-group-policy-round-trip-byte-equality`, `sagemaker.model-package-group-policy-unicode-byte-round-trip`, `sagemaker.model-package-group-policy-unicode-round-trip`, `sagemaker.put-model-package-group-policy-concurrent-last-writer-wins`, `sagemaker.put-model-package-group-policy-concurrent-overwrite`, `sagemaker.put-model-package-group-policy-concurrent-overwrite-deterministic` (+3 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/put_model_package_group_policy.rs`. Replaying the corpus, the emulator refused the `PutModelPackageGroupPolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sagemaker_concurrent_put_model_package_group_policy_last_writer_wins.py", line 69, in run
    assert len(errors) < 2, f"both concurrent PutModelPackageGroupPolicy calls failed: {errors}"
           ^^^^^^^^^^^^^^^
AssertionError: both concurrent PutModelPackageGroupPolicy calls failed: [ClientError('An error occurred (NotImplemented) when calling the PutModelPackageGroupPolicy operation: operation PutModelPackageGroupPolicy is scaffolded but not implemented in this emulator'), ClientError('An error occurred (NotImplemented) when calling the PutModelPackageGroupPolicy operation: operation PutModelPackageGroupPolicy is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the sagemaker emulator
Call PutModelPackageGroupPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
