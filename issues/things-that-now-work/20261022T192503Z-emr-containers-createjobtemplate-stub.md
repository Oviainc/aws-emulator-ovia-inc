Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.job-template-client-token-reuse-conflict-rejected emr-containers.job-template-create-idempotent-client-token emr-containers.job-template-delete-twice-rejected emr-containers.job-template-lifecycle-create-describe-delete emr-containers.job-template-tags-round-trip emr-containers.list-job-templates-pagination
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|CreateJobTemplate|NotImplemented
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement emr-containers CreateJobTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJobTemplate

## Description
`CreateJobTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/create_job_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateJobTemplate` call, so each one unblocks only when `CreateJobTemplate` works: `emr-containers.job-template-client-token-reuse-conflict-rejected`, `emr-containers.job-template-create-idempotent-client-token`, `emr-containers.job-template-delete-twice-rejected`, `emr-containers.job-template-lifecycle-create-describe-delete`, `emr-containers.job-template-tags-round-trip`, `emr-containers.list-job-templates-pagination`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/create_job_template.rs`. Replaying the corpus, the emulator refused the `CreateJobTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateJobTemplate operation: operation CreateJobTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the emr-containers emulator
Call CreateJobTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
