Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.cross-account-get-table-bucket-encryption-denied s3tables.delete-table-bucket-encryption-then-get-reverts-to-aes256-default s3tables.delete-table-bucket-encryption-with-active-tables-succeeds s3tables.e2e-table-bucket-replication-and-tagging-flow s3tables.e2e-table-lifecycle-with-encryption-and-replication s3tables.encryption-kms-key-arn-round-trip s3tables.get-table-bucket-encryption-without-permission-denied s3tables.put-and-get-table-bucket-encryption s3tables.put-table-bucket-encryption-lifecycle s3tables.put-table-bucket-encryption-then-delete-then-get-reverts-to-default s3tables.server-side-bucket-encryption-inherited-by-table s3tables.table-bucket-encryption-kms-key-arn-round-trip
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|PutTableBucketEncryption|NotImplemented
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# Implement s3tables PutTableBucketEncryption: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableBucketEncryption

## Description
`PutTableBucketEncryption` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_encryption.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `PutTableBucketEncryption` call, so each one unblocks only when `PutTableBucketEncryption` works: `s3tables.cross-account-get-table-bucket-encryption-denied`, `s3tables.delete-table-bucket-encryption-then-get-reverts-to-aes256-default`, `s3tables.delete-table-bucket-encryption-with-active-tables-succeeds`, `s3tables.e2e-table-bucket-replication-and-tagging-flow`, `s3tables.e2e-table-lifecycle-with-encryption-and-replication`, `s3tables.encryption-kms-key-arn-round-trip`, `s3tables.get-table-bucket-encryption-without-permission-denied`, `s3tables.put-and-get-table-bucket-encryption`, `s3tables.put-table-bucket-encryption-lifecycle`, `s3tables.put-table-bucket-encryption-then-delete-then-get-reverts-to-default`, `s3tables.server-side-bucket-encryption-inherited-by-table`, `s3tables.table-bucket-encryption-kms-key-arn-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_bucket_encryption.rs`. Replaying the corpus, the emulator refused the `PutTableBucketEncryption` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutTableBucketEncryption operation: operation PutTableBucketEncryption is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3tables emulator
Call PutTableBucketEncryption and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
