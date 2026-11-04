Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: supplychain.create-dataset-duplicate-name-conflict-then-new-name-succeeds supplychain.dataset-create-get-list-roundtrip supplychain.dataset-update-schema-description-persists supplychain.delete-namespace-with-dataset-conflict-then-succeeds supplychain.flow-create-get-roundtrip-with-config supplychain.flow-update-transformation-persists supplychain.namespace-create-get-list-roundtrip supplychain.send-data-integration-event-dataset-target-succeeds supplychain.send-event-dataset-target-missing-ref-not-found
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: supplychain|CreateDataLakeNamespace|NotImplemented
Issue-Commit: 0ebbd81840eb1d8236e49c909db275b2251d47fa

# Implement supplychain CreateDataLakeNamespace: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0ebbd81840eb1d8236e49c909db275b2251d47fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataLakeNamespace

## Description
`CreateDataLakeNamespace` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/create_data_lake_namespace.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreateDataLakeNamespace` call, so each one unblocks only when `CreateDataLakeNamespace` works: `supplychain.create-dataset-duplicate-name-conflict-then-new-name-succeeds`, `supplychain.dataset-create-get-list-roundtrip`, `supplychain.dataset-update-schema-description-persists`, `supplychain.delete-namespace-with-dataset-conflict-then-succeeds`, `supplychain.flow-create-get-roundtrip-with-config`, `supplychain.flow-update-transformation-persists`, `supplychain.namespace-create-get-list-roundtrip`, `supplychain.send-data-integration-event-dataset-target-succeeds`, `supplychain.send-event-dataset-target-missing-ref-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/create_data_lake_namespace.rs`. Replaying the corpus, the emulator refused the `CreateDataLakeNamespace` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDataLakeNamespace operation: operation CreateDataLakeNamespace is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the supplychain emulator
Call CreateDataLakeNamespace and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
