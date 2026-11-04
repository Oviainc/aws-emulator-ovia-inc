Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.create-data-grant-ends-at-past-rejected dataexchange.create-data-grant-invalid-receiver-principal-rejected
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|ListDataGrants|NotImplemented
Issue-Commit: 99a6bf4e8a9dd64f73b61f622317c845b92ac2b7

# Implement dataexchange ListDataGrants: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `99a6bf4e8a9dd64f73b61f622317c845b92ac2b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDataGrants

## Description
`ListDataGrants` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-dataexchange-dataexchange/src/ops/list_data_grants.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListDataGrants` call, so each one unblocks only when `ListDataGrants` works: `dataexchange.create-data-grant-ends-at-past-rejected`, `dataexchange.create-data-grant-invalid-receiver-principal-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-dataexchange-dataexchange/src/ops/list_data_grants.rs`. Replaying the corpus, the emulator refused the `ListDataGrants` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListDataGrants operation: operation ListDataGrants is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the dataexchange emulator
Call ListDataGrants and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
