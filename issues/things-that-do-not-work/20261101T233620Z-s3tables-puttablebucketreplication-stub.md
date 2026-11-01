Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.concurrent-put-table-bucket-replication-conflict s3tables.delete-table-bucket-replication-then-get-rejected s3tables.e2e-table-bucket-replication-lifecycle s3tables.put-and-get-table-bucket-replication s3tables.put-table-bucket-replication-concurrent-version-token-conflict s3tables.put-table-bucket-replication-conflicting-concurrent-updates s3tables.put-table-bucket-replication-lifecycle s3tables.put-table-bucket-replication-then-delete-then-get-rejected
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|PutTableBucketReplication|NotImplemented
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# Implement s3tables PutTableBucketReplication: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableBucketReplication

## Description
`PutTableBucketReplication` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_replication.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `PutTableBucketReplication` call, so each one unblocks only when `PutTableBucketReplication` works: `s3tables.concurrent-put-table-bucket-replication-conflict`, `s3tables.delete-table-bucket-replication-then-get-rejected`, `s3tables.e2e-table-bucket-replication-lifecycle`, `s3tables.put-and-get-table-bucket-replication`, `s3tables.put-table-bucket-replication-concurrent-version-token-conflict`, `s3tables.put-table-bucket-replication-conflicting-concurrent-updates`, `s3tables.put-table-bucket-replication-lifecycle`, `s3tables.put-table-bucket-replication-then-delete-then-get-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_replication.rs`. Replaying the corpus, the emulator refused the `PutTableBucketReplication` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutTableBucketReplication operation: operation PutTableBucketReplication is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3tables emulator
Call PutTableBucketReplication and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
