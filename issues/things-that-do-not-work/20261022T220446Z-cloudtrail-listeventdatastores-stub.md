Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.cancel-query-conflict-when-already-finished cloudtrail.cancel-query-unknown-event-data-store-fails cloudtrail.dashboard-status-transitions-through-refresh cloudtrail.event-data-store-notfound-crud-error-semantics cloudtrail.event-data-store-notfound-ingestion-error-semantics cloudtrail.event-data-store-retention-timestamp-precision-preserved cloudtrail.generate-query-rejects-unknown-event-data-store cloudtrail.list-event-data-stores-invalid-max-results-fails cloudtrail.restore-active-event-data-store-rejected cloudtrail.update-event-data-store-concurrent-conflict
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|ListEventDataStores|NotImplemented
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement cloudtrail ListEventDataStores: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEventDataStores

## Description
`ListEventDataStores` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/list_event_data_stores.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `ListEventDataStores` call, so each one unblocks only when `ListEventDataStores` works: `cloudtrail.cancel-query-conflict-when-already-finished`, `cloudtrail.cancel-query-unknown-event-data-store-fails`, `cloudtrail.dashboard-status-transitions-through-refresh`, `cloudtrail.event-data-store-notfound-crud-error-semantics`, `cloudtrail.event-data-store-notfound-ingestion-error-semantics`, `cloudtrail.event-data-store-retention-timestamp-precision-preserved`, `cloudtrail.generate-query-rejects-unknown-event-data-store`, `cloudtrail.list-event-data-stores-invalid-max-results-fails`, `cloudtrail.restore-active-event-data-store-rejected`, `cloudtrail.update-event-data-store-concurrent-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/list_event_data_stores.rs`. Replaying the corpus, the emulator refused the `ListEventDataStores` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListEventDataStores operation: operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.ListEventDataStores is not implemented in this emulator

## Steps to Reproduce
Boot the cloudtrail emulator
Call ListEventDataStores and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
