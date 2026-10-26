Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.put-lookup-table-malformed-schema-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|DeleteLookupTable|NotImplemented
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Implement logs DeleteLookupTable: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLookupTable

## Description
`DeleteLookupTable` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_lookup_table.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteLookupTable` call, so each one unblocks only when `DeleteLookupTable` works: `logs.put-lookup-table-malformed-schema-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_lookup_table.rs`. Replaying the corpus, the emulator refused the `DeleteLookupTable` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteLookupTable operation: operation DeleteLookupTable is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call DeleteLookupTable and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
