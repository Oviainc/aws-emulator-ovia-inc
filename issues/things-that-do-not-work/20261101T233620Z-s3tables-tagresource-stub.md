Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.concurrent-tag-untag-resource-consistent s3tables.e2e-bucket-namespace-table-replicate-tag-teardown s3tables.e2e-table-record-expiration-flow s3tables.e2e-tagging-lifecycle-across-resources s3tables.table-bucket-tags-unicode-round-trip s3tables.table-record-expiration-configuration-round-trip s3tables.tag-resource-and-list-tags-round-trip s3tables.tag-resource-unicode-round-trip s3tables.unicode-tag-value-round-trip s3tables.untag-resource-malformed-tag-key-rejected s3tables.untag-resource-nonexistent-tag-is-noop s3tables.untag-resource-nonexistent-tag-key-noop s3tables.untag-resource-then-list-tags-empty
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|TagResource|NotImplemented
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# Implement s3tables TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `s3tables.concurrent-tag-untag-resource-consistent`, `s3tables.e2e-bucket-namespace-table-replicate-tag-teardown`, `s3tables.e2e-table-record-expiration-flow`, `s3tables.e2e-tagging-lifecycle-across-resources`, `s3tables.table-bucket-tags-unicode-round-trip`, `s3tables.table-record-expiration-configuration-round-trip`, `s3tables.tag-resource-and-list-tags-round-trip`, `s3tables.tag-resource-unicode-round-trip`, `s3tables.unicode-tag-value-round-trip`, `s3tables.untag-resource-malformed-tag-key-rejected`, `s3tables.untag-resource-nonexistent-tag-is-noop`, `s3tables.untag-resource-nonexistent-tag-key-noop`, `s3tables.untag-resource-then-list-tags-empty`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3tables emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
