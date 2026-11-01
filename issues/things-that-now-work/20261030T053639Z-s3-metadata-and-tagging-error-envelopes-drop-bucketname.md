Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bucket-metadata-configuration-crud-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-bucket-metadata-configuration-crud-lifecycle s3-metadata-config-with-inventory-and-annotation s3-metadata-config-with-sse-encryption s3-metadata-configuration-journal-and-inventory s3-metadata-configuration-lifecycle s3-metadata-configuration-missing-config-errors s3-metadata-v2-config-lifecycle s3-bucket-replication-lifecycle s3-bucket-tagging-cors-website-logging s3-bucket-tagging-cost-allocation s3-bucket-tagging-lifecycle s3-bulk-object-delete-and-bucket-service-settings s3-metadata-configuration-duplicate-create-error s3-metadata-update-on-missing-config-errors s3-object-versioning-and-lock s3-object-annotations-lifecycle s3w-object-annotation-validation-errors
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketMetadataConfiguration|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketReplication|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketTagging|4xx->4xx|$.Error.BucketName
Issue-Key: s3|CreateBucketMetadataConfiguration|4xx->4xx|$.Error.BucketName
Issue-Key: s3|UpdateBucketMetadataJournalTableConfiguration|4xx->4xx|$.Error.BucketName
Issue-Key: s3|UpdateBucketMetadataInventoryTableConfiguration|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetObjectLockConfiguration|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetObjectAnnotation|4xx->4xx|$.Error.AnnotationName
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 metadata and tagging error envelopes drop BucketName and AnnotationName

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketMetadataConfiguration

## Description
Eight net-new root causes sharing one fault: the refusals produced by the S3 metadata-configuration, replication, tagging, object-lock and object-annotation handlers omit the contextual member AWS puts in the error envelope to name the resource that was not found. This is the same serializer gap as the bucket sub-resource issue filed alongside it, but these are separate handlers with their own error construction, so each must be fixed. A fixer must close all eight.

AWS (common): the S3 error document for a resource-scoped refusal is <Error><Code/><Message/><resource-naming member/><RequestId/><HostId/></Error>. Code and Message are the fixed text of the error class; the resource-naming member (BucketName for bucket-scoped errors, AnnotationName for annotation-scoped ones) is a variable part copied from the request. Smithy: the S3 model's error shapes (NoSuchBucket, NoSuchKey, NoSuchUpload, BucketAlreadyOwnedByYou) declare no members, and ReplicationConfigurationNotFoundError / MetadataConfigurationNotFound are not modelled at all, so these envelope members are recorded behavior only.

Why (common): the envelope is the only place the refusal says which resource it is about. A client driving several buckets or annotations through the same code path cannot attribute the failure, and SDK error shapes that expose BucketName/AnnotationName hand back None.

Cause 1 of 8: GetBucketMetadataConfiguration's 404 omits BucketName.
Emulator: 404 body has Code/Message/RequestId/HostId only.
Fix: the GetBucketMetadataConfiguration handler's not-found error construction.
Traces: s3-bucket-metadata-configuration-crud-lifecycle#task#5 (also s3-metadata-config-with-inventory-and-annotation#task#6, s3-metadata-config-with-sse-encryption#task#5, s3-metadata-configuration-journal-and-inventory#task#5, s3-metadata-configuration-lifecycle#task#2, s3-metadata-configuration-lifecycle#task#6, s3-metadata-configuration-missing-config-errors#task#2, s3-metadata-v2-config-lifecycle#task#5).

Cause 2 of 8: GetBucketReplication's 404 ReplicationConfigurationNotFoundError omits BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetBucketReplication handler's not-found error construction.
Traces: s3-bucket-replication-lifecycle#task#9.

Cause 3 of 8: GetBucketTagging's 404 NoSuchTagSet omits BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetBucketTagging handler's not-found error construction.
Traces: s3-bucket-tagging-cors-website-logging#task#7 (also s3-bucket-tagging-cost-allocation#task#7, s3-bucket-tagging-lifecycle#task#5, s3-bulk-object-delete-and-bucket-service-settings#task#7, s3-bulk-object-delete-and-bucket-service-settings#task#11).

Cause 4 of 8: CreateBucketMetadataConfiguration's 409 duplicate-configuration conflict omits BucketName.
Emulator: $.Error.BucketName absent on the 409.
Fix: the CreateBucketMetadataConfiguration handler's conflict error construction.
Traces: s3-metadata-configuration-duplicate-create-error#task#3.

Cause 5 of 8: UpdateBucketMetadataJournalTableConfiguration's 404 omits BucketName.
Emulator: $.Error.BucketName absent.
Fix: the UpdateBucketMetadataJournalTableConfiguration handler's not-found error construction.
Traces: s3-metadata-update-on-missing-config-errors#task#2.

Cause 6 of 8: UpdateBucketMetadataInventoryTableConfiguration's 404 omits BucketName.
Emulator: $.Error.BucketName absent.
Fix: the UpdateBucketMetadataInventoryTableConfiguration handler's not-found error construction.
Traces: s3-metadata-update-on-missing-config-errors#task#3.

Cause 7 of 8: GetObjectLockConfiguration's 404 ObjectLockConfigurationNotFoundError omits BucketName.
Emulator: $.Error.BucketName absent.
Fix: the GetObjectLockConfiguration handler's not-found error construction.
Traces: s3-object-versioning-and-lock#task#13.

Cause 8 of 8: GetObjectAnnotation's 404 omits AnnotationName.
AWS: the annotation-scoped 404 envelope carries <AnnotationName> whose value is the annotation name from the request's query/header.
Emulator: $.Error.AnnotationName absent.
Fix: the GetObjectAnnotation handler's not-found error construction.
Traces: s3-object-annotations-lifecycle#task#7 (also s3w-object-annotation-validation-errors#task#5, s3w-object-annotation-validation-errors#task#11)

## Evidence
All eight clusters are 'mismatch' with matching status; only the envelope member diverged.
s3-bucket-metadata-configuration-crud-lifecycle#task#5 (GetBucketMetadataConfiguration, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-replication-lifecycle#task#9 (GetBucketReplication, 404 -> 404): $.Error.BucketName: missing (got None)
s3-bucket-tagging-cors-website-logging#task#7 (GetBucketTagging, 404 -> 404): $.Error.BucketName: missing (got None)
s3-metadata-configuration-duplicate-create-error#task#3 (CreateBucketMetadataConfiguration, 409 -> 409): $.Error.BucketName: missing (got None)
s3-metadata-update-on-missing-config-errors#task#2 (UpdateBucketMetadataJournalTableConfiguration, 404 -> 404): $.Error.BucketName: missing (got None)
s3-metadata-update-on-missing-config-errors#task#3 (UpdateBucketMetadataInventoryTableConfiguration, 404 -> 404): $.Error.BucketName: missing (got None)
s3-object-versioning-and-lock#task#13 (GetObjectLockConfiguration, 404 -> 404): $.Error.BucketName: missing (got None)
s3-object-annotations-lifecycle#task#7 (GetObjectAnnotation, 404 -> 404): $.Error.AnnotationName: missing (expected the requested annotation name, got None)

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: GET /<bucket>?metadataConfiguration, ?replication, ?tagging and ?object-lock on a bucket with no such configuration; POST CreateBucketMetadataConfiguration twice for the same bucket; call UpdateBucketMetadataJournalTableConfiguration and UpdateBucketMetadataInventoryTableConfiguration on a bucket with no metadata configuration; GET an object annotation that does not exist. Inspect each refusal's <Error> element: it must carry <BucketName> (or <AnnotationName> for the annotation path) naming the resource from the request.
