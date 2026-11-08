Issue-Source: trace-sweep-divergence
Issue-Trace: s3-cfg-configure-before-create
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-cfg-configure-before-create s3-cfg-tags-at-create s3-cfg-us-east-1-location-constraint-mistake s3-cfg-us-east-1-reference-path s3-lsrno-traj-bucket-region-addressing s3-oc-create-bucket-ownership-header-mistakes s3-cob-continuation-token-is-a-reusable-name-cursor s3-cob-deleted-bucket-drops-out-of-the-count s3-cob-listing-orders-by-name-not-creation-date s3-cob-max-buckets-value-mistakes s3-cob-prefix-scoped-listing-and-prefix-mistakes s3-itc-list-buckets-continuation-token-reflects-mid-walk-inserts s3-sseinv-get-encryption-bucket-reference-mistakes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CreateBucket|2xx->2xx|$headers.x-amz-bucket-arn
Issue-Key: s3|CreateBucket|4xx->4xx|$.Error.Code
Issue-Key: s3|CreateBucket|4xx->4xx|$.Error.LocationConstraint
Issue-Key: s3|CreateBucket|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketTagging|2xx->4xx|$.Tagging
Issue-Key: s3|GetBucketTagging|2xx->4xx|status
Issue-Key: s3|ListBuckets|2xx->2xx|$.ListAllMyBucketsResult.Buckets.Bucket.CreationDate
Issue-Key: s3|ListBuckets|2xx->2xx|$.ListAllMyBucketsResult.Buckets.Bucket[Name="sim2real-probe-cob-0e91840b-pg-mm"].CreationDate
Issue-Key: s3|ListBuckets|2xx->2xx|$.ListAllMyBucketsResult.Buckets.Bucket[Name="sim2real-probe-cob-0e91840b-pg-zz"].CreationDate
Issue-Key: s3|ListBuckets|2xx->2xx|$.ListAllMyBucketsResult.Buckets.Bucket.BucketArn
Issue-Key: s3|ListBuckets|2xx->2xx|$.ListAllMyBucketsResult.Buckets.Bucket.Name
Issue-Key: s3|ListBuckets|2xx->2xx|$.ListAllMyBucketsResult.ContinuationToken
Issue-Key: s3|GetBucketEncryption|4xx->4xx|$.Error.Code
Issue-Key: s3|GetBucketEncryption|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketEncryption|4xx->4xx|status
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 bucket create list and tagging contracts diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucket

## Description
Cause 1 of 5: CreateBucket omits x-amz-bucket-arn. AWS: a successful CreateBucket answers 200 with the response header x-amz-bucket-arn = arn:aws:s3:::<bucket>. Recorded behavior only. Emulator: the header is absent. Why: callers (and CloudFormation-style wrappers) that take the created bucket's ARN from the response have to synthesise it. Fix: the CreateBucket response header assembly. Traces: s3-cfg-configure-before-create#task#3 (cluster of 7 steps).

Cause 2 of 5: an invalid LocationConstraint is reported as a schema fault. AWS: 400 Code 'InvalidLocationConstraint', Message 'The specified location-constraint is not valid' with <LocationConstraint> = the value as sent. Recorded behavior only. Emulator: 400 MalformedXML with the generic schema sentence and no LocationConstraint member. Why: the caller is told its XML is malformed when the document is well formed and the region value is wrong - the us-east-1 special case in particular. Fix: validate LocationConstraint after parsing in the CreateBucket handler. Traces: s3-cfg-us-east-1-location-constraint-mistake#task#1.

Cause 3 of 5: tags supplied at bucket creation are not stored. AWS: GetBucketTagging after a create that carried tags answers 200 with the full TagSet. Recorded behavior only. Emulator: 404 and no Tagging element. Why: a create-with-tags workflow believes its tags were applied; cost-allocation and policy-by-tag then see an untagged bucket. Fix: carry the create-time tagging into the bucket tag store in state. Traces: s3-cfg-tags-at-create#task#2.

Cause 4 of 5: ListBuckets CreationDate carries sub-millisecond precision. AWS: bucket CreationDate is an ISO-8601 instant in S3's XML date-time form; the recorded responses are asserted by shape, which the emulator's microsecond form ('2026-11-02T10:44:23.276968Z') does not satisfy. Recorded behavior only. Why: consumers that parse S3's fixed-width timestamp form (and fixtures that compare by shape) reject the value. Fix: the ListBuckets timestamp serializer. Traces: s3-cob-continuation-token-is-a-reusable-name-cursor#task#4 (cluster of 9 steps), s3-cob-continuation-token-is-a-reusable-name-cursor#task#7.

Cause 5 of 5: the ListBuckets name cursor skips a bucket and omits ContinuationToken. AWS: paging by continuation-token is a reusable name cursor - the page beginning after the previous one starts at the next name in order (bucket2), and the response echoes <ContinuationToken> for the page that was requested with one. Recorded behavior only. Emulator: the page starts at bucket3 (one bucket is skipped) and ContinuationToken is absent. Why: a paging walk silently loses buckets, so inventory is incomplete. Fix: the ListBuckets pagination cursor in the listing handler. Traces: s3-itc-list-buckets-continuation-token-reflects-mid-walk-inserts#task#6.

Also bundled: GetBucketEncryption on a syntactically invalid bucket name answers the wrong class. AWS: 400 Code 'InvalidBucketName', Message 'The specified bucket is not valid.' Recorded behavior only. Emulator: 404 NoSuchBucket 'The specified bucket does not exist'. Why: a client cannot tell a malformed name from an absent bucket, so it retries creation with a name that can never exist. Fix: bucket-name syntax validation in the shared bucket-request pre-handler. Traces: s3-sseinv-get-encryption-bucket-reference-mistakes#task#5.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-cfg-configure-before-create#task#3 (CreateBucket 200/200): $headers.x-amz-bucket-arn missing (expected 'arn:aws:s3:::<bucket>'), cluster of 7 steps. s3-cfg-us-east-1-location-constraint-mistake#task#1: Code expected 'InvalidLocationConstraint' got 'MalformedXML', Message expected 'The specified location-constraint is not valid', LocationConstraint member missing. s3-cfg-tags-at-create#task#2: status expected 200 with the four-tag TagSet, got 404. s3-cob-continuation-token-is-a-reusable-name-cursor#task#4: CreationDate value_mismatch against the shaped expectation, got '2026-11-02T10:44:23.276968Z' (cluster of 9). s3-itc-list-buckets-continuation-token-reflects-mid-walk-inserts#task#6: Bucket.Name expected '...-bucket2' got '...-bucket3', BucketArn likewise, ContinuationToken missing. s3-sseinv-get-encryption-bucket-reference-mistakes#task#5: status expected 400 InvalidBucketName, got 404 NoSuchBucket.

## Steps to Reproduce
1. Replay s3-cfg-configure-before-create step task#3 and inspect the response headers. 2. Replay s3-cfg-us-east-1-location-constraint-mistake step task#1. 3. Replay s3-cfg-tags-at-create step task#2. 4. Replay s3-cob-continuation-token-is-a-reusable-name-cursor steps task#4/task#7 and s3-itc-list-buckets-continuation-token-reflects-mid-walk-inserts step task#6. 5. Replay s3-sseinv-get-encryption-bucket-reference-mistakes step task#5.
