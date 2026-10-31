Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule ec2__iam__imagebuilder.ibp-delete-image-is-rejected-while-its-build-runs ec2__iam__imagebuilder.ibp-pipeline-run-against-a-missing-security-group-fails-and-a-valid-one-builds
Issue-Key: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule|run#9|AssertionError|assert (code, message) == ("InvalidParameterValueException", expected), (
Issue-Key: imagebuilder|CreateImagePipeline|NotImplemented
Issue-Commit: 86d4487805fae2c305c5dd9de933831005de30fd

# Implement imagebuilder CreateImagePipeline: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `86d4487805fae2c305c5dd9de933831005de30fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImagePipeline

## Description
`CreateImagePipeline` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/create_image_pipeline.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateImagePipeline` call, so each one unblocks only when `CreateImagePipeline` works: `ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule`, `ec2__iam__imagebuilder.ibp-delete-image-is-rejected-while-its-build-runs`, `ec2__iam__imagebuilder.ibp-pipeline-run-against-a-missing-security-group-fails-and-a-valid-one-builds`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/create_image_pipeline.rs`. Replaying the corpus, the emulator refused the `CreateImagePipeline` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_ec2__iam__imagebuilder_ibp_create_image_pipeline_rejects_an_invalid_cron_schedule.py", line 38, in run
    assert (code, message) == ("InvalidParameterValueException", expected), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: 'cron(not valid)' should be refused InvalidParameterValueException "The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid schedule expression. Details: Invalid CRON expression: error at position 0, details: Illegal characters for this position: 'NOT'", got 'NotImplemented' 'operation CreateImagePipeline is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the imagebuilder emulator
Call CreateImagePipeline and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
