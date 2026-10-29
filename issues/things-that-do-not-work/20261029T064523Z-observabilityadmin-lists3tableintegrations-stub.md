Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.s3-table-integration-associate-list-disassociate
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: observabilityadmin|ListS3TableIntegrations|NotImplemented
Issue-Commit: 6bf56903357d8bdb827814619137753aca334420

# Implement observabilityadmin ListS3TableIntegrations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6bf56903357d8bdb827814619137753aca334420`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListS3TableIntegrations

## Description
`ListS3TableIntegrations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/list_s3_table_integrations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListS3TableIntegrations` call, so each one unblocks only when `ListS3TableIntegrations` works: `logs.s3-table-integration-associate-list-disassociate`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/list_s3_table_integrations.rs`. Replaying the corpus, the emulator refused the `ListS3TableIntegrations` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListS3TableIntegrations operation: operation ListS3TableIntegrations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the observabilityadmin emulator
Call ListS3TableIntegrations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
