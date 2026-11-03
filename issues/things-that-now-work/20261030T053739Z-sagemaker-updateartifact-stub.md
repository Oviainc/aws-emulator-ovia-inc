Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.artifact-create-describe-delete sagemaker.artifact-describe-after-delete-rejected
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.artifact-create-describe-delete|run#19|AssertionError|assert update_err["Code"] == "ResourceNotFound", (
Issue-Key: sagemaker.artifact-describe-after-delete-rejected|run#11|AssertionError|assert update_err["Code"] == "ResourceNotFound", (
Issue-Commit: bd260fbb92f697f6565f73ef409cec21a7b2614a

# Implement sagemaker UpdateArtifact: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bd260fbb92f697f6565f73ef409cec21a7b2614a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateArtifact

## Description
`UpdateArtifact` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_artifact.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateArtifact` call, so each one unblocks only when `UpdateArtifact` works: `sagemaker.artifact-create-describe-delete`, `sagemaker.artifact-describe-after-delete-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_artifact.rs`. Replaying the corpus, the emulator refused the `UpdateArtifact` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_sagemaker_artifact_create_describe_delete.py", line 62, in run
    assert update_err["Code"] == "ResourceNotFound", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected UpdateArtifact on a deleted artifact to fail ResourceNotFound, got {'Message': 'operation UpdateArtifact is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the sagemaker emulator
Call UpdateArtifact and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
