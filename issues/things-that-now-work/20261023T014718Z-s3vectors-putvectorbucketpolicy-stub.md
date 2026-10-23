Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.delete-vector-bucket-policy-removes-it s3vectors.delete-vector-bucket-policy-then-get-404 s3vectors.put-vector-bucket-policy-lifecycle s3vectors.put-vector-bucket-policy-malformed-json-rejected s3vectors.put-vector-bucket-policy-overwrite-replaces-not-merges s3vectors.put-vector-bucket-policy-then-get-echoes s3vectors.vector-bucket-policy-document-round-trip s3vectors.vector-bucket-policy-enforced-denies-list-vectors-rejected s3vectors.vector-bucket-policy-enforced-on-access-denied s3vectors.vector-bucket-policy-round-trip
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|PutVectorBucketPolicy|NotImplemented
Issue-Key: s3vectors.put-vector-bucket-policy-malformed-json-rejected|run#7|AssertionError|assert err["Code"] == "ValidationException", f"expected ValidationException for malformed policy JSON, got {err!r}"
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# Implement s3vectors PutVectorBucketPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutVectorBucketPolicy

## Description
`PutVectorBucketPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/put_vector_bucket_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `PutVectorBucketPolicy` call, so each one unblocks only when `PutVectorBucketPolicy` works: `s3vectors.delete-vector-bucket-policy-removes-it`, `s3vectors.delete-vector-bucket-policy-then-get-404`, `s3vectors.put-vector-bucket-policy-lifecycle`, `s3vectors.put-vector-bucket-policy-malformed-json-rejected`, `s3vectors.put-vector-bucket-policy-overwrite-replaces-not-merges`, `s3vectors.put-vector-bucket-policy-then-get-echoes`, `s3vectors.vector-bucket-policy-document-round-trip`, `s3vectors.vector-bucket-policy-enforced-denies-list-vectors-rejected`, `s3vectors.vector-bucket-policy-enforced-on-access-denied`, `s3vectors.vector-bucket-policy-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/put_vector_bucket_policy.rs`. Replaying the corpus, the emulator refused the `PutVectorBucketPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutVectorBucketPolicy operation: operation PutVectorBucketPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3vectors emulator
Call PutVectorBucketPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
