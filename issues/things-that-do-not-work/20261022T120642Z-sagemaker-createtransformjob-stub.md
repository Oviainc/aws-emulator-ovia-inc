Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cli-sagemaker-batch-transform-s3-workflow s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive s3__sagemaker.sagemaker-batch-transform-writes-output-s3-positive
Issue-Key: s3__sagemaker.cli-sagemaker-batch-transform-s3-workflow|_create_via_cli_with_role_retry#17|AssertionError|raise AssertionError(f"{operation} failed: {proc.stderr[-500:]}")
Issue-Key: sagemaker|CreateTransformJob|NotImplemented
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker CreateTransformJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTransformJob

## Description
`CreateTransformJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_transform_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateTransformJob` call, so each one unblocks only when `CreateTransformJob` works: `s3__sagemaker.cli-sagemaker-batch-transform-s3-workflow`, `s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive`, `s3__sagemaker.sagemaker-batch-transform-writes-output-s3-positive`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_transform_job.rs`. Replaying the corpus, the emulator refused the `CreateTransformJob` call: File "check_s3__sagemaker_cli_sagemaker_batch_transform_s3_workflow.py", line 143, in run
    _create_via_cli_with_role_retry(ctx, "sagemaker", "create-transform-job", transform_payload)
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_s3__sagemaker_cli_sagemaker_batch_transform_s3_workflow.py", line 323, in _create_via_cli_with_role_retry
    raise AssertionError(f"{operation} failed: {proc.stderr[-500:]}")
AssertionError: create-transform-job failed: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateTransformJob operation: operation CreateTransformJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sagemaker emulator
Call CreateTransformJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
