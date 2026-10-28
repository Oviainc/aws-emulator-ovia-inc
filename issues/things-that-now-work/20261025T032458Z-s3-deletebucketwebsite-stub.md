Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.get-bucket-website-not-configured-rejected s3.s3-website-hosting-delete-on-an-unconfigured-bucket-is-a-quiet-no-op s3.s3-website-hosting-derivation-selects-exactly-the-website-configured-buckets
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|DeleteBucketWebsite|NotImplemented
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# Implement s3 DeleteBucketWebsite: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBucketWebsite

## Description
`DeleteBucketWebsite` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket_website.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteBucketWebsite` call, so each one unblocks only when `DeleteBucketWebsite` works: `s3.get-bucket-website-not-configured-rejected`, `s3.s3-website-hosting-delete-on-an-unconfigured-bucket-is-a-quiet-no-op`, `s3.s3-website-hosting-derivation-selects-exactly-the-website-configured-buckets`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket_website.rs`. Replaying the corpus, the emulator refused the `DeleteBucketWebsite` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteBucketWebsite operation: operation DeleteBucketWebsite is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3 emulator
Call DeleteBucketWebsite and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
