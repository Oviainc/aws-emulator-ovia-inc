Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bap-bucket-reference-mistakes
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-bap-bucket-reference-mistakes s3-cfoac-oac-grant-policy-document-mistakes s3-cfoac-public-shortcut-blocked-then-service-principal-grants s3-cfoac-tenant-bucket-deny-only-policy-alternate-reads s3-pab-block-public-policy-classifies-the-document s3-aaxe-data-export-schema-mistakes s3-b2b-lifecycle-rule-document-mistakes s3-lcp-traj-put-lifecycle-refusals-then-recover s3-lctf-traj-tag-filter-document-mistakes s3-itc-lifecycle-transition-is-not-a-tiering-configuration s3-lcp-traj-bucket-addressing-mistakes s3-lcp-traj-disabled-only-policy-and-idempotent-delete s3-lcp-traj-reference-classify-by-get-status s3-lcp-traj-stale-bucket-after-delete s3-lctf-traj-reference-walk-tag-vs-prefix-vs-none s3-itc-put-filter-and-id-mistakes s3-itc-put-tiering-day-and-enum-mistakes s3-itc-read-mistakes-on-the-per-bucket-probe s3-oc-bucket-reference-mistakes s3-oc-enforce-blocked-by-bucket-acl-then-recovered s3-oc-put-document-mistakes-then-corrected s3-pab-enforced-bucket-acl-refusal-order s3-wsh-delete-website-idempotent-vs-missing-bucket s3-wsh-index-and-error-document-refusals-then-recover s3-wsh-redirect-all-exclusivity-refusals-then-recover s3-wsh-reference-walk-counts-index-and-redirect-all-buckets s3-pab-absent-block-vs-absent-bucket s3-pab-expected-bucket-owner-guard s3-pab-acl-enabled-bucket-block-public-acls-classifies-grants s3-sseinv-get-encryption-bucket-reference-mistakes s3-sseinv-put-encryption-algorithm-and-key-mistakes s3-cob-deleted-bucket-drops-out-of-the-count s3-mbes-traj-plain-delete-then-version-purge s3-ovm-traj-delete-marker-as-most-recent-entry s3-ovm-traj-list-objects-v2-then-head-for-version s3-ovm-traj-list-versions-parameter-mistakes s3-ovm-traj-overwrite-latest-filter-and-noncurrent-size s3-ovm-traj-prefix-narrowing-and-attributes-route s3-ovm-traj-reference-latest-versions s3-ovm-traj-version-id-argument-mistakes s3-met-traj-get-by-id-absent-vs-present s3-metrics-cfg-list-key-reverts-to-omitted-after-last-delete s3-met-traj-config-id-rules-then-corrected s3-mbes-traj-bucket-reference-mistakes s3-mbes-traj-reference-version-purge s3-mbes-traj-delete-objects-batch-mistakes s3-mbes-traj-versioning-status-and-null-version s3-aaxe-read-mistakes-on-the-per-bucket-probe s3-aaxe-inventory-export-is-not-an-analytics-export s3-aaxe-reput-replaces-and-deleting-the-foreign-arm s3-csbs-sibling-key-and-bucket-reference-mistakes s3-lctf-traj-bucket-tags-are-not-filter-tags s3-oc-create-bucket-ownership-header-mistakes s3w-checksum-mode-get-object-exact s3w-put-object-checksum-mismatch-rejected s3-wsh-routing-rule-refusals-then-recover s3-refarch-s3ev-bucket-addressing-and-filter-name-mistakes s3-b2b-summary-metadata-is-not-tagging
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketPolicyStatus|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketPolicy|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketLifecycleConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|ListBucketIntelligentTieringConfigurations|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketOwnershipControls|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketOwnershipControls|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketAcl|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketWebsite|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketWebsite|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeleteBucketWebsite|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetPublicAccessBlock|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeletePublicAccessBlock|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutPublicAccessBlock|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketLocation|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeleteBucket|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketMetricsConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketMetricsConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeleteObjects|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketVersioning|4xx->4xx|$.Error.RequestId
Issue-Key: s3|ListBucketAnalyticsConfigurations|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketAnalyticsConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeleteBucketAnalyticsConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketPolicy|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeleteBucketLifecycle|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketTagging|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetBucketNotificationConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutObjectTagging|4xx->4xx|$.Error.RequestId
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 request ids are UUIDs instead of AWS request-id tokens

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketPolicyStatus

## Description
Cause 1 of 1: the request identifier S3 mints for every response has the wrong shape, on every operation and in both the x-amz-request-id header and the <RequestId> element of an <Error> body.

AWS: an S3 request id is a fixed-width, 16-character uppercase alphanumeric token (recorded examples: 'X92SNP8YKTXS6CAT', 'SM9KCM9XCV2VY89B', '7E2DMTA841Y2C0BZ'); the paired HostId is a long base64 token. The value itself is per-request, but its SHAPE is part of the wire contract, which is why the corpus asserts it as shaped-not-asserted rather than as a literal. Recorded behavior only - the Smithy model does not model the S3 error envelope.

Emulator: it emits a lowercase RFC-4122 UUID ('87f04608-44b0-4187-b857-c4373ef7fe71') and a 64-hex HostId, so the shape assertion fails on every refusal in the corpus (this single cause accounts for the large majority of this service's mismatch clusters; each listed fixture#step is one such cluster).

Why: client and proxy code that parses, logs or length-checks S3 request ids - and support tooling that expects to quote a 16-character token - mis-handles a 36-character UUID; recordings and fixtures captured against the emulator also become distinguishable from AWS by shape alone.

Fix: the shared response-id generator in the S3 request/response plumbing (mint 16 uppercase alphanumeric characters for x-amz-request-id / <RequestId>, and an AWS-shaped x-amz-id-2 / <HostId>), not any single operation handler.

Traces: s3-bap-bucket-reference-mistakes#task#3 plus every fixture#step listed in trace_ids - each is a distinct replay cluster whose only divergence is the request-id shape.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-bap-bucket-reference-mistakes#task#3 (GetBucketPolicyStatus, expected 404 / actual 404): $.Error.RequestId value_mismatch (expected '<shaped-not-asserted>', got '87f04608-44b0-4187-b857-c4373ef7fe71'). Where the recording pinned a literal AWS token the contrast is explicit: s3-b2b-summary-metadata-is-not-tagging#task#5 expected 'X92SNP8YKTXS6CAT' and got 'f0da0c8c-e652-43ec-9154-531fead0d7d6'; s3-mbes-traj-plain-delete-then-version-purge#task#8 expected '7E2DMTA841Y2C0BZ' and got '77dac0f6-e9f5-4e5f-8c89-76f7fb843a1a'. Status, code and message matched in these clusters; only the id shape diverged.

## Steps to Reproduce
1. Replay s3-bap-bucket-reference-mistakes and inspect step task#3's <Error><RequestId> and the x-amz-request-id header. 2. Observe a 36-character UUID where AWS returns a 16-character uppercase alphanumeric token. 3. Repeat for any fixture#step in trace_ids.
