Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bucket-abac-and-session
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-bucket-abac-and-session s3-bucket-abac-status-lifecycle s3-bucket-access-policy-and-public-access-controls s3-bucket-policy-and-ownership s3-bucket-policy-lifecycle s3-bucket-encryption-and-public-access s3-public-access-block-security-hardening s3-bucket-ownership-controls-for-acl-migration s3-bucket-acl-versioning-payer s3-bucket-lifecycle-basics s3-bucket-configuration-crud s3-bucket-cors-lifecycle s3-bucket-tagging-cors-website-logging s3-cors-configuration-lifecycle s3-bucket-lifecycle-config-lifecycle s3-bucket-lifecycle-notifications-replication s3-object-lifecycle-rules-management s3-bucket-website-lifecycle s3-static-website-hosting-configuration s3-bucket-lifecycle-and-discovery s3-bucket-lifecycle-core
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketAbac|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketPolicy|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetPublicAccessBlock|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketOwnershipControls|4xx->4xx|$.Error.BucketName
Issue-Key: s3|DeleteBucket|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketCors|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketLifecycleConfiguration|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketWebsite|4xx->4xx|$.Error.BucketName
Issue-Key: s3|CreateBucket|4xx->4xx|$.Error.BucketName
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 bucket error envelopes omit the BucketName member

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketAbac

## Description
Ten net-new root causes sharing one fault: the S3 REST-XML error serializer emits only Code, Message, RequestId and HostId, and drops the bucket-naming member that AWS places in every bucket-scoped error envelope. Each bucket sub-resource handler builds its own refusal, so each one has to be fixed. A fixer must close all ten.

AWS (common to all ten): a bucket-scoped S3 error document is <Error><Code/><Message/><BucketName/><RequestId/><HostId/></Error>. The fixed text is the Code and Message for that error class; BucketName is a variable part taken verbatim from the request's bucket (the host or first path segment), and RequestId/HostId are per-request values. Smithy: com.amazonaws.s3#NoSuchBucket (404) and com.amazonaws.s3#BucketAlreadyOwnedByYou (409) model no members at all, so the envelope's contextual members are recorded behavior only; every recording in this corpus shows <BucketName> present.

Why (common to all ten): a client that fans out one operation over many buckets (or that logs/retries per bucket) identifies which bucket failed from the envelope's BucketName. Without it the refusal is unattributable, and SDK error objects that surface `BucketName` hand back None.

Cause 1 of 10: GetBucketAbac answers 404 'NoSuchBucket' / 'The specified bucket does not exist.' with no BucketName.
Emulator: body carries Code/Message/RequestId/HostId only; $.Error.BucketName is absent.
Fix: the GetBucketAbac handler's not-found error construction and the shared S3 error serializer.
Traces: s3-bucket-abac-and-session#task#3 (also s3-bucket-abac-status-lifecycle#teardown#8).

Cause 2 of 10: GetBucketPolicy answers 404 with no BucketName.
Emulator: $.Error.BucketName absent on the NoSuchBucket 404.
Fix: the GetBucketPolicy handler's error construction.
Traces: s3-bucket-access-policy-and-public-access-controls#task#6 (also s3-bucket-policy-and-ownership#task#8, s3-bucket-policy-lifecycle#task#6).

Cause 3 of 10: GetPublicAccessBlock answers 404 with no BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetPublicAccessBlock handler's error construction.
Traces: s3-bucket-access-policy-and-public-access-controls#task#10 (also s3-bucket-encryption-and-public-access#task#10, s3-public-access-block-security-hardening#task#7).

Cause 4 of 10: GetBucketOwnershipControls answers 404 with no BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetBucketOwnershipControls handler's error construction.
Traces: s3-bucket-access-policy-and-public-access-controls#task#14 (also s3-bucket-ownership-controls-for-acl-migration#task#7, s3-bucket-policy-and-ownership#task#12).

Cause 5 of 10: DeleteBucket answers 404 with no BucketName.
Emulator: $.Error.BucketName absent on the NoSuchBucket 404.
Fix: the DeleteBucket handler's not-found error construction.
Traces: s3-bucket-acl-versioning-payer#teardown#13 (also s3-bucket-lifecycle-basics#teardown#8).

Cause 6 of 10: GetBucketCors answers 404 with no BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetBucketCors handler's error construction.
Traces: s3-bucket-configuration-crud#task#5 (also s3-bucket-cors-lifecycle#task#5, s3-bucket-tagging-cors-website-logging#task#11, s3-cors-configuration-lifecycle#task#7).

Cause 7 of 10: GetBucketLifecycleConfiguration answers 404 with no BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetBucketLifecycleConfiguration handler's error construction.
Traces: s3-bucket-configuration-crud#task#13 (also s3-bucket-lifecycle-config-lifecycle#task#5, s3-bucket-lifecycle-notifications-replication#task#8, s3-object-lifecycle-rules-management#task#7).

Cause 8 of 10: GetBucketWebsite answers 404 with no BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetBucketWebsite handler's error construction.
Traces: s3-bucket-configuration-crud#task#21 (also s3-bucket-tagging-cors-website-logging#task#15, s3-bucket-website-lifecycle#task#5, s3-static-website-hosting-configuration#task#7).

Cause 9 of 10: CreateBucket's 409 BucketAlreadyOwnedByYou omits BucketName.
AWS: the 409 conflict envelope also names the bucket the caller already owns.
Emulator: $.Error.BucketName absent on the 409.
Fix: the CreateBucket handler's conflict error construction.
Traces: s3-bucket-lifecycle-and-discovery#task#8 (also s3-bucket-lifecycle-basics#task#5, s3-bucket-lifecycle-core#task#9).

Cause 10 of 10: DeleteBucket's 409 BucketNotEmpty omits BucketName.
Emulator: $.Error.BucketName absent on the 409.
Fix: the DeleteBucket handler's not-empty conflict error construction.
Traces: s3-bucket-lifecycle-and-discovery#task#10

## Evidence
Replay verdict 'mismatch' on all ten clusters; the status matched and only the envelope member diverged.
s3-bucket-abac-and-session#task#3 (GetBucketAbac, 404 -> 404): $.Error.BucketName: missing (expected the request's bucket name, got None)
s3-bucket-access-policy-and-public-access-controls#task#6 (GetBucketPolicy, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-access-policy-and-public-access-controls#task#10 (GetPublicAccessBlock, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-access-policy-and-public-access-controls#task#14 (GetBucketOwnershipControls, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-acl-versioning-payer#teardown#13 (DeleteBucket, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-configuration-crud#task#5 (GetBucketCors, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-configuration-crud#task#13 (GetBucketLifecycleConfiguration, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-configuration-crud#task#21 (GetBucketWebsite, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-lifecycle-and-discovery#task#8 (CreateBucket, 409 -> 409): $.Error.BucketName: missing (got None)
s3-bucket-lifecycle-and-discovery#task#10 (DeleteBucket, 409 -> 409): $.Error.BucketName: missing (got None)

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: create a bucket, then (a) GET /<bucket>?abac, ?policy, ?publicAccessBlock, ?ownershipControls, ?cors, ?lifecycle and ?website on a bucket name that does not exist and inspect the 404 XML body; (b) DELETE a non-existent bucket; (c) CreateBucket twice for the same name and inspect the 409; (d) DeleteBucket on a bucket that still holds objects and inspect the 409. In every case the <Error> element must contain a <BucketName> child whose value is the requested bucket.
