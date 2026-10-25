Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.deep-s3-storage-class-lifecycle-transition-colder-before-warmer-rejected s3.deep-s3-storage-class-lifecycle-transition-glacier-to-deep-archive-gap-rejected s3.e2e-move-s3-contents-and-cleanup-lifecycle s3.lifecycle-abort-incomplete-multipart-invalid-days-rejected s3.op-contract-delete-bucket-lifecycle s3.s3-conditional-precedence-and-lifecycle-headers-lifecycle-mpu s3.s3-lcp-cli-exit-status-classifies-buckets-with-and-without-a-policy s3.s3-lcp-expiration-header-follows-tag-and-size-filters s3.s3-lcp-object-expiration-header-names-the-earliest-matching-rule s3.s3-lcp-omitted-rule-ids-are-generated-distinct-and-stable s3.s3-lcp-solve-predicate-selects-only-non-excluded-lifecycle-buckets s3.s3-lifecycle-tag-bucket-walk-finds-only-the-bucket-with-a-tag-filtered-expiration s3.s3-lifecycle-tag-cli-upstream-query-selects-the-tag-filtered-expiration-rules s3.s3-lifecycle-tag-disabled-rule-stamps-nothing-while-its-enabled-twin-does s3.s3-lifecycle-tag-expiry-date-is-upload-day-plus-days-rounded-to-the-next-utc-midnight s3.s3-lifecycle-tag-legacy-prefix-rule-beside-a-filter-rule-rejected s3.s3-lifecycle-tag-more-than-one-thousand-rules-rejected s3.s3-lifecycle-tag-omitted-rule-id-is-generated-unique-and-stable s3.s3-lifecycle-tag-put-and-copy-responses-carry-the-expiration-of-a-tagged-object s3.s3-lifecycle-tag-retagging-an-object-moves-it-into-and-out-of-the-rule
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketLifecycleConfiguration|NotImplemented
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# Implement s3 PutBucketLifecycleConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketLifecycleConfiguration

## Description
`PutBucketLifecycleConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_lifecycle_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `PutBucketLifecycleConfiguration` call, so each one unblocks only when `PutBucketLifecycleConfiguration` works: `s3.deep-s3-storage-class-lifecycle-transition-colder-before-warmer-rejected`, `s3.deep-s3-storage-class-lifecycle-transition-glacier-to-deep-archive-gap-rejected`, `s3.e2e-move-s3-contents-and-cleanup-lifecycle`, `s3.lifecycle-abort-incomplete-multipart-invalid-days-rejected`, `s3.op-contract-delete-bucket-lifecycle`, `s3.s3-conditional-precedence-and-lifecycle-headers-lifecycle-mpu`, `s3.s3-lcp-cli-exit-status-classifies-buckets-with-and-without-a-policy`, `s3.s3-lcp-expiration-header-follows-tag-and-size-filters`, `s3.s3-lcp-object-expiration-header-names-the-earliest-matching-rule`, `s3.s3-lcp-omitted-rule-ids-are-generated-distinct-and-stable`, `s3.s3-lcp-solve-predicate-selects-only-non-excluded-lifecycle-buckets`, `s3.s3-lifecycle-tag-bucket-walk-finds-only-the-bucket-with-a-tag-filtered-expiration`, `s3.s3-lifecycle-tag-cli-upstream-query-selects-the-tag-filtered-expiration-rules`, `s3.s3-lifecycle-tag-disabled-rule-stamps-nothing-while-its-enabled-twin-does`, `s3.s3-lifecycle-tag-expiry-date-is-upload-day-plus-days-rounded-to-the-next-utc-midnight`, `s3.s3-lifecycle-tag-legacy-prefix-rule-beside-a-filter-rule-rejected`, `s3.s3-lifecycle-tag-more-than-one-thousand-rules-rejected`, `s3.s3-lifecycle-tag-omitted-rule-id-is-generated-unique-and-stable`, `s3.s3-lifecycle-tag-put-and-copy-responses-carry-the-expiration-of-a-tagged-object`, `s3.s3-lifecycle-tag-retagging-an-object-moves-it-into-and-out-of-the-rule`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_lifecycle_configuration.rs`. Replaying the corpus, the emulator refused the `PutBucketLifecycleConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutBucketLifecycleConfiguration operation: operation PutBucketLifecycleConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3 emulator
Call PutBucketLifecycleConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
