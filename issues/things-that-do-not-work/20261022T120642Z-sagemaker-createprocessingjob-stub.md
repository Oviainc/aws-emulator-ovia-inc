Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cli-sagemaker-processing-job-s3-output-verify s3__sagemaker.sagemaker-processing-job-kms-encrypted-output-positive s3__sagemaker.sagemaker-processing-job-kms-encrypted-s3-output-positive s3__sagemaker.sagemaker-processing-job-missing-s3-list-permission-negative s3__sagemaker.sagemaker-processing-job-missing-s3-putobject-permission-negative s3__sagemaker.sagemaker-processing-job-missing-s3-write-permission-negative s3__sagemaker.sagemaker-processing-job-output-data-format-preserved s3__sagemaker.sagemaker-processing-job-reads-s3-input s3__sagemaker.sagemaker-processing-job-reads-s3-input-positive s3__sagemaker.sagemaker-processing-job-s3-bucket-policy-denies-negative s3__sagemaker.sagemaker-processing-job-s3-data-distribution-type-preserved s3__sagemaker.sagemaker-processing-job-s3-input-positive s3__sagemaker.sagemaker-processing-job-s3-output-bucket-policy-blocks-write-negative s3__sagemaker.sagemaker-processing-job-s3-output-positive s3__sagemaker.sagemaker-processing-job-writes-output-to-s3-positive s3__sagemaker.sagemaker-processing-job-writes-s3-output s3__sagemaker.sagemaker-processing-job-writes-s3-output-positive sagemaker.processing-job-managed-terminal-state
Issue-Key: s3__sagemaker.cli-sagemaker-processing-job-s3-output-verify|_run_aws_create_with_role_retry#9|AssertionError|raise AssertionError(f"{service} {operation} failed: {proc.stderr[-500:]}")
Issue-Key: sagemaker|CreateProcessingJob|NotImplemented
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker CreateProcessingJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProcessingJob

## Description
`CreateProcessingJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_processing_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

18 recorded unit(s) fail on their `CreateProcessingJob` call, so each one unblocks only when `CreateProcessingJob` works: `s3__sagemaker.cli-sagemaker-processing-job-s3-output-verify`, `s3__sagemaker.sagemaker-processing-job-kms-encrypted-output-positive`, `s3__sagemaker.sagemaker-processing-job-kms-encrypted-s3-output-positive`, `s3__sagemaker.sagemaker-processing-job-missing-s3-list-permission-negative`, `s3__sagemaker.sagemaker-processing-job-missing-s3-putobject-permission-negative`, `s3__sagemaker.sagemaker-processing-job-missing-s3-write-permission-negative`, `s3__sagemaker.sagemaker-processing-job-output-data-format-preserved`, `s3__sagemaker.sagemaker-processing-job-reads-s3-input`, `s3__sagemaker.sagemaker-processing-job-reads-s3-input-positive`, `s3__sagemaker.sagemaker-processing-job-s3-bucket-policy-denies-negative`, `s3__sagemaker.sagemaker-processing-job-s3-data-distribution-type-preserved`, `s3__sagemaker.sagemaker-processing-job-s3-input-positive`, `s3__sagemaker.sagemaker-processing-job-s3-output-bucket-policy-blocks-write-negative`, `s3__sagemaker.sagemaker-processing-job-s3-output-positive`, `s3__sagemaker.sagemaker-processing-job-writes-output-to-s3-positive`, `s3__sagemaker.sagemaker-processing-job-writes-s3-output`, `s3__sagemaker.sagemaker-processing-job-writes-s3-output-positive`, `sagemaker.processing-job-managed-terminal-state`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_processing_job.rs`. Replaying the corpus, the emulator refused the `CreateProcessingJob` call: File "check_s3__sagemaker_cli_sagemaker_processing_job_s3_output_verify.py", line 79, in run
    _run_aws_create_with_role_retry(ctx, "sagemaker", "create-processing-job", payload)
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_s3__sagemaker_cli_sagemaker_processing_job_s3_output_verify.py", line 320, in _run_aws_create_with_role_retry
    raise AssertionError(f"{service} {operation} failed: {proc.stderr[-500:]}")
AssertionError: sagemaker create-processing-job failed: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateProcessingJob operation: operation CreateProcessingJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sagemaker emulator
Call CreateProcessingJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
