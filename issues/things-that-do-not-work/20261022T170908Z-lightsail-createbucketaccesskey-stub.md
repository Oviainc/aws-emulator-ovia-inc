Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.bucket-access-key-lifecycle lightsail.bucket-delete-nonempty-without-force-rejected lightsail.bucket-object-put-get-roundtrip
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|CreateBucketAccessKey|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement lightsail CreateBucketAccessKey: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucketAccessKey

## Description
`CreateBucketAccessKey` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_bucket_access_key.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateBucketAccessKey` call, so each one unblocks only when `CreateBucketAccessKey` works: `lightsail.bucket-access-key-lifecycle`, `lightsail.bucket-delete-nonempty-without-force-rejected`, `lightsail.bucket-object-put-get-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_bucket_access_key.rs`. Replaying the corpus, the emulator refused the `CreateBucketAccessKey` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateBucketAccessKey operation: operation CreateBucketAccessKey is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lightsail emulator
Call CreateBucketAccessKey and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
