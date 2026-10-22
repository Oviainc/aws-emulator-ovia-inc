Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.ai-recommendation-job-create-delete-lifecycle sagemaker.ai-workload-config-create-delete-lifecycle sagemaker.delete-ai-workload-config-idempotent-not-found-rejected sagemaker.delete-ai-workload-config-in-use-blocks-delete sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateAIWorkloadConfig|NotImplemented
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker CreateAIWorkloadConfig: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAIWorkloadConfig

## Description
`CreateAIWorkloadConfig` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_ai_workload_config.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateAIWorkloadConfig` call, so each one unblocks only when `CreateAIWorkloadConfig` works: `sagemaker.ai-recommendation-job-create-delete-lifecycle`, `sagemaker.ai-workload-config-create-delete-lifecycle`, `sagemaker.delete-ai-workload-config-idempotent-not-found-rejected`, `sagemaker.delete-ai-workload-config-in-use-blocks-delete`, `sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_ai_workload_config.rs`. Replaying the corpus, the emulator refused the `CreateAIWorkloadConfig` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAIWorkloadConfig operation: operation CreateAIWorkloadConfig is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sagemaker emulator
Call CreateAIWorkloadConfig and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
