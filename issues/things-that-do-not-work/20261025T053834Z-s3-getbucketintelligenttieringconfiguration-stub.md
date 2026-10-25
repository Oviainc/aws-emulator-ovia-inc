Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.bucket-intelligent-tiering-configuration-lifecycle s3.bucket-intelligent-tiering-filtered-configs-replace-not-duplicate s3.s3-itc-and-filter-of-prefix-and-tag-round-trips-with-both-archive-tiers s3.s3-itc-get-per-id-matches-each-list-entry s3.s3-itc-put-rejected-for-a-config-id-over-sixty-four-characters s3.s3-itc-put-rejected-for-tier-days-above-seven-hundred-thirty
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketIntelligentTieringConfiguration|NotImplemented
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# Implement s3 GetBucketIntelligentTieringConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketIntelligentTieringConfiguration

## Description
`GetBucketIntelligentTieringConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/get_bucket_intelligent_tiering_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `GetBucketIntelligentTieringConfiguration` call, so each one unblocks only when `GetBucketIntelligentTieringConfiguration` works: `s3.bucket-intelligent-tiering-configuration-lifecycle`, `s3.bucket-intelligent-tiering-filtered-configs-replace-not-duplicate`, `s3.s3-itc-and-filter-of-prefix-and-tag-round-trips-with-both-archive-tiers`, `s3.s3-itc-get-per-id-matches-each-list-entry`, `s3.s3-itc-put-rejected-for-a-config-id-over-sixty-four-characters`, `s3.s3-itc-put-rejected-for-tier-days-above-seven-hundred-thirty`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/get_bucket_intelligent_tiering_configuration.rs`. Replaying the corpus, the emulator refused the `GetBucketIntelligentTieringConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetBucketIntelligentTieringConfiguration operation: operation GetBucketIntelligentTieringConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3 emulator
Call GetBucketIntelligentTieringConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
