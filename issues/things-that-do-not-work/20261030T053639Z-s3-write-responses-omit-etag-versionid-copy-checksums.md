Issue-Source: trace-sweep-divergence
Issue-Trace: s3w-checksum-mode-get-object-exact
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3w-checksum-mode-get-object-exact s3w-copy-object-metadata-directive s3w-object-system-metadata-roundtrip s3w-precondition-chain s3w-put-object-checksum-mismatch-rejected s3w-object-lock-governance-delete s3-copy-object-lifecycle s3-object-copy-and-rename s3-object-crud-and-metadata s3-bucket-lifecycle-basics s3-bucket-lifecycle-core
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|2xx->2xx|$.ETag
Issue-Key: s3|PutObject|2xx->2xx|$.VersionId
Issue-Key: s3|CopyObject|2xx->2xx|$.CopyObjectResult.ChecksumCRC32
Issue-Key: s3|CopyObject|2xx->2xx|$.CopyObjectResult.ChecksumType
Issue-Key: s3|HeadBucket|2xx->2xx|$headers.x-amz-bucket-arn
Issue-Key: s3|ListBuckets|2xx->2xx|$.ListAllMyBucketsResult.Buckets.Bucket.BucketArn
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 write responses omit ETag, VersionId, copy checksums and bucket ARNs

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Six net-new root causes sharing one fault: successful S3 writes and bucket reads hand back a 200 with the identity of what was written stripped out — no ETag, no VersionId, no copy checksum, no bucket ARN. A fixer must close all six.

Why (common): these members are the result of the call, not decoration. Without an ETag from PutObject a client cannot do an optimistic If-Match update, cannot populate a multipart part list, and cannot verify the write; without VersionId it cannot address the version it just created; without the copy checksum it cannot verify a copy; without the bucket ARN it cannot build a policy, tag request or cross-service reference for the bucket it just looked up.

Cause 1 of 6: PutObject omits the ETag output member.
AWS: PutObject returns the stored object's entity tag. Smithy: PutObjectOutput.ETag is bound to the ETag response header.
Emulator: the ETag member is absent from the response.
Fix: the PutObject serializer and the ETag the write path computes.
Traces: s3w-checksum-mode-get-object-exact#task#2 (also #task#3, s3w-copy-object-metadata-directive#task#2, s3w-object-system-metadata-roundtrip#setup#2, s3w-precondition-chain#task#2, #task#5, s3w-put-object-checksum-mismatch-rejected#task#2).

Cause 2 of 6: PutObject on a versioning-enabled bucket omits both ETag and VersionId.
AWS: PutObjectOutput.VersionId is bound to x-amz-version-id and names the version just created.
Emulator: both absent.
Fix: the PutObject serializer, on the versioned path.
Traces: s3w-object-lock-governance-delete#task#4.

Cause 3 of 6: CopyObject omits ChecksumCRC32 and ChecksumType from <CopyObjectResult>.
AWS: CopyObject computes the checksum over the copied bytes and returns it in the result body with ChecksumType FULL_OBJECT.
Emulator: both members absent.
Fix: the CopyObject handler's checksum computation and its result serializer. (The open behavioral issue 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md, cause 10, describes CopyObject checksum computation but carries no wire replay key for this step.)
Traces: s3-copy-object-lifecycle#task#3 (also s3-object-copy-and-rename#task#3, s3w-copy-object-metadata-directive#task#3, #task#5, s3w-precondition-chain#task#11).

Cause 4 of 6: CopyObject omits ChecksumType even when a checksum value is present.
Emulator: $.CopyObjectResult.ChecksumType absent.
Fix: as cause 3 — ChecksumType must accompany the copied-object checksum.
Traces: s3-object-crud-and-metadata#task#6.

Cause 5 of 6: HeadBucket omits the x-amz-bucket-arn response header.
AWS: HeadBucket returns the bucket's ARN. Smithy: HeadBucketOutput.BucketArn is bound to httpHeader x-amz-bucket-arn with pattern ^arn:[^:]+:(s3|s3express):.
Emulator: the header is absent.
Fix: the HeadBucket serializer and the bucket state it reads, which must carry the bucket's ARN.
Traces: s3-bucket-lifecycle-basics#task#2 (also s3-bucket-lifecycle-core#task#2).

Cause 6 of 6: ListBuckets entries omit BucketArn.
AWS: each <Bucket> carries Name, CreationDate and BucketArn. Smithy: com.amazonaws.s3#Bucket models BucketArn.
Emulator: BucketArn absent from the listing entry.
Fix: the ListBuckets serializer and the bucket state it reads. The open behavioral issue 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md, cause 7, records the same gap but carries no wire replay key for this step.
Traces: s3-bucket-lifecycle-core#task#6

## Evidence
All six clusters are 'mismatch' with matching status 200.
s3w-checksum-mode-get-object-exact#task#2 (PutObject): $.ETag missing (expected the stored object's entity tag, got None)
s3w-object-lock-governance-delete#task#4 (PutObject): $.ETag missing; $.VersionId missing (expected the newly created version id)
s3-copy-object-lifecycle#task#3 (CopyObject): $.CopyObjectResult.ChecksumCRC32 missing; $.CopyObjectResult.ChecksumType missing (expected 'FULL_OBJECT')
s3-object-crud-and-metadata#task#6 (CopyObject): $.CopyObjectResult.ChecksumType missing (expected 'FULL_OBJECT')
s3-bucket-lifecycle-basics#task#2 (HeadBucket): $headers.x-amz-bucket-arn missing (expected 'arn:aws:s3:::<bucket>')
s3-bucket-lifecycle-core#task#6 (ListBuckets): $.ListAllMyBucketsResult.Buckets.Bucket.BucketArn missing (expected 'arn:aws:s3:::<bucket>')

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: PutObject on a plain bucket and check the ETag response header; PutObject on a versioning-enabled bucket and check ETag plus x-amz-version-id; CopyObject with x-amz-sdk-checksum-algorithm CRC32 and check <CopyObjectResult> carries ChecksumCRC32 and ChecksumType; HeadBucket and check x-amz-bucket-arn; a parameterless ListBuckets and check each <Bucket> carries <BucketArn>.
