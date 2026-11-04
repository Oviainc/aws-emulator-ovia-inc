Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.athena-data-catalog-glue-create-complete-state athena.data-catalog-delete-reserved-aws-data-catalog-fails athena.data-catalog-lifecycle athena.data-catalog-list-contains-created athena.list-data-catalogs-includes-built-in-and-created athena.list-data-catalogs-includes-builtin-and-created athena.list-data-catalogs-includes-default-deterministically athena.list-data-catalogs-pagination-completeness athena.op-contract-get-data-catalog
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|ListDataCatalogs|NotImplemented
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Implement athena ListDataCatalogs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDataCatalogs

## Description
`ListDataCatalogs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/list_data_catalogs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `ListDataCatalogs` call, so each one unblocks only when `ListDataCatalogs` works: `athena.athena-data-catalog-glue-create-complete-state`, `athena.data-catalog-delete-reserved-aws-data-catalog-fails`, `athena.data-catalog-lifecycle`, `athena.data-catalog-list-contains-created`, `athena.list-data-catalogs-includes-built-in-and-created`, `athena.list-data-catalogs-includes-builtin-and-created`, `athena.list-data-catalogs-includes-default-deterministically`, `athena.list-data-catalogs-pagination-completeness`, `athena.op-contract-get-data-catalog`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/list_data_catalogs.rs`. Replaying the corpus, the emulator refused the `ListDataCatalogs` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListDataCatalogs operation: operation ListDataCatalogs is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call ListDataCatalogs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
