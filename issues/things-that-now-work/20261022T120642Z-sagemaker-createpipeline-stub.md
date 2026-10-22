Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-pipeline-cyclic-dependson-rejected sagemaker.create-pipeline-missing-dependency-not-found sagemaker.create-pipeline-missing-role-not-found sagemaker.pipeline-condition-step-outcome-selects-exactly-one-declared-branch sagemaker.pipeline-create-describe-delete sagemaker.pipeline-create-describe-delete-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreatePipeline|NotImplemented
Issue-Key: sagemaker.create-pipeline-missing-dependency-not-found|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message}"
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker CreatePipeline: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipeline

## Description
`CreatePipeline` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_pipeline.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreatePipeline` call, so each one unblocks only when `CreatePipeline` works: `sagemaker.create-pipeline-cyclic-dependson-rejected`, `sagemaker.create-pipeline-missing-dependency-not-found`, `sagemaker.create-pipeline-missing-role-not-found`, `sagemaker.pipeline-condition-step-outcome-selects-exactly-one-declared-branch`, `sagemaker.pipeline-create-describe-delete`, `sagemaker.pipeline-create-describe-delete-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_pipeline.rs`. Replaying the corpus, the emulator refused the `CreatePipeline` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePipeline operation: operation CreatePipeline is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sagemaker emulator
Call CreatePipeline and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
