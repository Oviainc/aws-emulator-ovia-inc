Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.athena-delete-data-catalog-nonexistent-rejected athena.athena-get-data-catalog-after-delete-not-found athena.concurrent-data-catalog-delete-only-one-succeeds athena.data-catalog-glue-create-synchronous-and-delete athena.data-catalog-parameters-map-round-trip athena.op-contract-deleteDataCatalog
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|DeleteDataCatalog|NotImplemented
Issue-Key: athena.concurrent-data-catalog-delete-only-one-succeeds|run#13|AssertionError|assert len(successes) >= 1, f"expected at least one delete to succeed, got {results}"
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Implement athena DeleteDataCatalog: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataCatalog

## Description
`DeleteDataCatalog` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/delete_data_catalog.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `DeleteDataCatalog` call, so each one unblocks only when `DeleteDataCatalog` works: `athena.athena-delete-data-catalog-nonexistent-rejected`, `athena.athena-get-data-catalog-after-delete-not-found`, `athena.concurrent-data-catalog-delete-only-one-succeeds`, `athena.data-catalog-glue-create-synchronous-and-delete`, `athena.data-catalog-parameters-map-round-trip`, `athena.op-contract-deleteDataCatalog`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/delete_data_catalog.rs`. Replaying the corpus, the emulator refused the `DeleteDataCatalog` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteDataCatalog operation: operation DeleteDataCatalog is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call DeleteDataCatalog and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
