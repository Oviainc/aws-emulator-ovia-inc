Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.concurrent-put-table-replication-conflict s3tables.delete-table-replication-stale-version-token-rejected s3tables.delete-table-replication-then-get-rejected s3tables.delete-table-replication-without-permission-denied s3tables.get-table-replication-status-reflects-enabled-replication s3tables.put-and-get-table-replication s3tables.put-table-replication-conflict-on-already-replicating-table s3tables.put-table-replication-cross-account-without-permission-denied s3tables.put-table-replication-lifecycle s3tables.put-table-replication-then-delete-then-get-rejected s3tables.replication-status-reflects-propagation s3tables.table-replication-status-reflects-pending-then-complete
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|PutTableReplication|NotImplemented
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# Implement s3tables PutTableReplication: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableReplication

## Description
`PutTableReplication` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_replication.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `PutTableReplication` call, so each one unblocks only when `PutTableReplication` works: `s3tables.concurrent-put-table-replication-conflict`, `s3tables.delete-table-replication-stale-version-token-rejected`, `s3tables.delete-table-replication-then-get-rejected`, `s3tables.delete-table-replication-without-permission-denied`, `s3tables.get-table-replication-status-reflects-enabled-replication`, `s3tables.put-and-get-table-replication`, `s3tables.put-table-replication-conflict-on-already-replicating-table`, `s3tables.put-table-replication-cross-account-without-permission-denied`, `s3tables.put-table-replication-lifecycle`, `s3tables.put-table-replication-then-delete-then-get-rejected`, `s3tables.replication-status-reflects-propagation`, `s3tables.table-replication-status-reflects-pending-then-complete`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_replication.rs`. Replaying the corpus, the emulator refused the `PutTableReplication` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutTableReplication operation: operation PutTableReplication is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3tables emulator
Call PutTableReplication and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
