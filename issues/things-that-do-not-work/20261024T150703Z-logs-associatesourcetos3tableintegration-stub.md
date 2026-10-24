Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.disassociate-source-unknown-identifier-rejected logs.s3-table-integration-associate-list-disassociate logs.s3-table-integration-source-association-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|AssociateSourceToS3TableIntegration|NotImplemented
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Implement logs AssociateSourceToS3TableIntegration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateSourceToS3TableIntegration

## Description
`AssociateSourceToS3TableIntegration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/associate_source_to_s3_table_integration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `AssociateSourceToS3TableIntegration` call, so each one unblocks only when `AssociateSourceToS3TableIntegration` works: `logs.disassociate-source-unknown-identifier-rejected`, `logs.s3-table-integration-associate-list-disassociate`, `logs.s3-table-integration-source-association-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/associate_source_to_s3_table_integration.rs`. Replaying the corpus, the emulator refused the `AssociateSourceToS3TableIntegration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociateSourceToS3TableIntegration operation: operation AssociateSourceToS3TableIntegration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call AssociateSourceToS3TableIntegration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
