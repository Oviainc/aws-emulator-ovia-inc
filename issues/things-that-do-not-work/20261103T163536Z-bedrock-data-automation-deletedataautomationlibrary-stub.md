Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-data-automation.create-data-automation-library-duplicate-name-conflict-rejected bedrock-data-automation.delete-data-automation-library-removes-resource bedrock-data-automation.delete-data-automation-library-twice-second-call-not-found-rejected bedrock-data-automation.library-delete-while-referenced-by-project-validation-rejected
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|DeleteDataAutomationLibrary|NotImplemented
Issue-Commit: 6788b77349a4fc1e05cc03a6c90d9f4cfe03594a

# Implement bedrock-data-automation DeleteDataAutomationLibrary: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6788b77349a4fc1e05cc03a6c90d9f4cfe03594a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataAutomationLibrary

## Description
`DeleteDataAutomationLibrary` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/delete_data_automation_library.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `DeleteDataAutomationLibrary` call, so each one unblocks only when `DeleteDataAutomationLibrary` works: `bedrock-data-automation.create-data-automation-library-duplicate-name-conflict-rejected`, `bedrock-data-automation.delete-data-automation-library-removes-resource`, `bedrock-data-automation.delete-data-automation-library-twice-second-call-not-found-rejected`, `bedrock-data-automation.library-delete-while-referenced-by-project-validation-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/delete_data_automation_library.rs`. Replaying the corpus, the emulator refused the `DeleteDataAutomationLibrary` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteDataAutomationLibrary operation: operation DeleteDataAutomationLibrary is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-data-automation emulator
Call DeleteDataAutomationLibrary and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
