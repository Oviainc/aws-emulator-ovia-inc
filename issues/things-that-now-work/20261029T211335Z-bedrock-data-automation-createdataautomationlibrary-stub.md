Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-data-automation.create-data-automation-library-active-immediately bedrock-data-automation.create-data-automation-library-duplicate-name-conflict-rejected bedrock-data-automation.delete-data-automation-library-removes-resource bedrock-data-automation.delete-data-automation-library-twice-second-call-not-found-rejected bedrock-data-automation.ingestion-job-delete-operation-removes-entity bedrock-data-automation.ingestion-job-inline-upsert-completes-and-entity-retrievable bedrock-data-automation.ingestion-job-invalid-entity-type-rejected bedrock-data-automation.ingestion-job-missing-required-phrases-validation-rejected bedrock-data-automation.invoke-ingestion-job-wrong-arn-type-rejected bedrock-data-automation.library-delete-while-referenced-by-project-validation-rejected bedrock-data-automation.list-data-automation-libraries-reflects-lifecycle bedrock-data-automation.list-data-automation-library-ingestion-jobs-empty-for-new-library bedrock-data-automation.list-ingestion-jobs-reflects-invocation bedrock-data-automation.update-data-automation-library-description-persists
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|CreateDataAutomationLibrary|NotImplemented
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# Implement bedrock-data-automation CreateDataAutomationLibrary: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataAutomationLibrary

## Description
`CreateDataAutomationLibrary` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/create_data_automation_library.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `CreateDataAutomationLibrary` call, so each one unblocks only when `CreateDataAutomationLibrary` works: `bedrock-data-automation.create-data-automation-library-active-immediately`, `bedrock-data-automation.create-data-automation-library-duplicate-name-conflict-rejected`, `bedrock-data-automation.delete-data-automation-library-removes-resource`, `bedrock-data-automation.delete-data-automation-library-twice-second-call-not-found-rejected`, `bedrock-data-automation.ingestion-job-delete-operation-removes-entity`, `bedrock-data-automation.ingestion-job-inline-upsert-completes-and-entity-retrievable`, `bedrock-data-automation.ingestion-job-invalid-entity-type-rejected`, `bedrock-data-automation.ingestion-job-missing-required-phrases-validation-rejected`, `bedrock-data-automation.invoke-ingestion-job-wrong-arn-type-rejected`, `bedrock-data-automation.library-delete-while-referenced-by-project-validation-rejected`, `bedrock-data-automation.list-data-automation-libraries-reflects-lifecycle`, `bedrock-data-automation.list-data-automation-library-ingestion-jobs-empty-for-new-library`, `bedrock-data-automation.list-ingestion-jobs-reflects-invocation`, `bedrock-data-automation.update-data-automation-library-description-persists`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/create_data_automation_library.rs`. Replaying the corpus, the emulator refused the `CreateDataAutomationLibrary` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDataAutomationLibrary operation: operation CreateDataAutomationLibrary is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-data-automation emulator
Call CreateDataAutomationLibrary and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
