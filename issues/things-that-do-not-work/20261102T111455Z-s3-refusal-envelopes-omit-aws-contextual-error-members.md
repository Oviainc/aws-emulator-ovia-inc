Issue-Source: trace-sweep-divergence
Issue-Trace: s3-cfg-bucket-tagging-refusals
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-cfg-bucket-tagging-refusals s3-b2b-summary-inline-tagging-header-mistakes s3-cob-max-buckets-value-mistakes s3-csbs-object-attributes-request-mistakes s3-rcsv-object-attributes-route-mistakes s3-lcp-traj-put-lifecycle-refusals-then-recover s3-mbes-traj-delete-objects-batch-mistakes s3-oc-create-bucket-ownership-header-mistakes s3-sseinv-put-encryption-algorithm-and-key-mistakes s3-csbs-sibling-key-and-bucket-reference-mistakes s3-ovm-traj-version-id-argument-mistakes s3-sync-versioned-source-stale-and-deleted s3-lsd-invalid-bucket-name-reads-as-missing s3-sseinv-get-encryption-bucket-reference-mistakes s3-csvcols-part-number-reads s3-rcsv-part-number-reads-on-single-part-csv s3-txt-part-number-download-on-a-single-part-text-file s3-mbes-traj-plain-delete-then-version-purge s3-ovm-traj-delete-marker-as-most-recent-entry s3-put-rejects-nonexistent-queue-arn s3-x2x-deleted-function-leaves-dangling-config s3-x2x-lambda-destination-reference-forms s3-txt-version-id-mistakes-on-versioned-and-unversioned-buckets
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketTagging|4xx->4xx|$.Error.TagKey
Issue-Key: s3|PutBucketTagging|4xx->4xx|$.Error.TagValue
Issue-Key: s3|PutObject|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutObject|4xx->4xx|$.Error.TagKey
Issue-Key: s3|ListBuckets|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|ListBuckets|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListBuckets|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketLifecycleConfiguration|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|PutBucketLifecycleConfiguration|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|PutBucketLifecycleConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|DeleteObject|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|DeleteObject|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|DeleteObject|4xx->4xx|$.Error.RequestId
Issue-Key: s3|CreateBucket|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|CreateBucket|4xx->4xx|$.Error.RequestId
Issue-Key: s3|PutBucketEncryption|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|PutBucketEncryption|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetObjectTagging|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.Key
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.VersionId
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.VersionId
Issue-Key: s3|ListObjectsV2|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketEncryption|4xx->4xx|$.Error.BucketName
Issue-Key: s3|GetBucketEncryption|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ActualPartCount
Issue-Key: s3|GetObject|4xx->4xx|$.Error.PartNumberRequested
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Method
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ResourceType
Issue-Key: s3|GetObject|4xx->4xx|$headers.last-modified
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.Method
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.ResourceType
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$headers.x-amz-delete-marker
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$headers.x-amz-version-id
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->4xx|$.Error.ArgumentName1
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->4xx|$.Error.ArgumentValue1
Issue-Key: s3|PutBucketNotificationConfiguration|4xx->4xx|$.Error.RequestId
Issue-Key: s3|GetObjectAttributes|4xx->4xx|$.Error.Message
Issue-Related: 20261030T053639Z-s3-object-error-envelopes-omit-key-and-versionid.md
Issue-Related: 20261030T053639Z-s3-refusals-use-the-wrong-error-code-and.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 refusal envelopes omit AWS contextual error members

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketTagging

## Description
S3 refusal bodies are an <Error> element that carries, besides Code/Message/RequestId/HostId, the per-error-class context members that name WHAT was rejected. The emulator emits only Code/Message/RequestId/HostId, so every one of those context members is absent. The Smithy model does not model the S3 error envelope's extra members at all (e.g. ListObjectsV2 declares only NoSuchBucket, GetObjectAttributes only NoSuchKey), so for each cause below the requirement is recorded behavior only: the member name is fixed text of that error class and its value is taken from the request or from emulator state, never copied from a recording.

Cause 1 of 9: tagging refusals omit TagKey/TagValue. AWS: a PutBucketTagging / PutObject(x-amz-tagging) tag-validation refusal carries <TagKey> (and <TagValue> when the value is what failed) naming the offending tag from the request. Recorded behavior only. Emulator: the 400 body has no TagKey/TagValue element. Why: a client that reports which tag the service rejected has nothing to report and cannot repair the request. Fix: the tagging validation path's error serializer. Traces: s3-cfg-bucket-tagging-refusals#task#2 (TagKey), s3-cfg-bucket-tagging-refusals#task#4 (TagKey+TagValue), s3-b2b-summary-inline-tagging-header-mistakes#task#2 (PutObject TagKey).

Cause 2 of 9: InvalidArgument refusals omit ArgumentName/ArgumentValue. AWS: every S3 InvalidArgument carries <ArgumentName> = the rejected parameter/header/XML element name and <ArgumentValue> = the value as sent. Recorded behavior only. Emulator: both absent. Why: clients and the CLI surface "which argument" from these members; without them the refusal is unattributable. Fix: the shared InvalidArgument constructor in the request-validation layer, so every call site supplies name+value. Traces: s3-cob-max-buckets-value-mistakes#task#5 (max-buckets/-1), s3-csbs-object-attributes-request-mistakes#task#4 (x-amz-object-attributes/objectsize), s3-lcp-traj-put-lifecycle-refusals-then-recover#task#3 (Days/0), s3-mbes-traj-delete-objects-batch-mistakes#task#9 (versionId), s3-oc-create-bucket-ownership-header-mistakes#task#1 (x-amz-object-ownership), s3-sseinv-put-encryption-algorithm-and-key-mistakes#task#5 (ApplyServerSideEncryptionByDefault), s3-txt-version-id-mistakes-on-versioned-and-unversioned-buckets#task#7 (versionId/empty).

Cause 3 of 9: object refusals omit Key and VersionId. AWS: NoSuchKey / NoSuchVersion / InvalidArgument on an object route carries <Key> = the requested key and, for a version-qualified request, <VersionId> = the requested version id. Recorded behavior only. Emulator: both absent on GetObjectTagging, GetObjectAttributes and CopyObject refusals. Why: in a batch or sync loop the caller cannot tell which key of many failed. Fix: the object-route error mapper, which already knows the resolved key/versionId. Traces: s3-b2b-summary-inline-tagging-header-mistakes#task#6, s3-csbs-sibling-key-and-bucket-reference-mistakes#task#8, s3-ovm-traj-version-id-argument-mistakes#task#6 (Key+VersionId), s3-sync-versioned-source-stale-and-deleted#task#7 (CopyObject Key+VersionId).

Cause 4 of 9: bucket refusals omit BucketName. AWS: NoSuchBucket and the bucket-scoped not-found/invalid refusals carry <BucketName> = the bucket as addressed. Recorded behavior only. Emulator: absent. Why: the same reason as Cause 3 for bucket-scoped sweeps. Fix: the bucket-route error mapper. Traces: s3-lsd-invalid-bucket-name-reads-as-missing#task#5, s3-sseinv-get-encryption-bucket-reference-mistakes#task#6.

Cause 5 of 9: 416 part-number refusals omit PartNumberRequested/ActualPartCount. AWS: a GetObject with partNumber beyond the object's part count answers 416 with <PartNumberRequested> = the requested number and <ActualPartCount> = the object's part count from state. Recorded behavior only. Emulator: status 416 is right, both members absent. Why: a ranged/parallel downloader uses ActualPartCount to bound its loop. Fix: the GetObject part-number path. Traces: s3-csvcols-part-number-reads#task#5.

Cause 6 of 9: 405 delete-marker refusals omit Method/ResourceType. AWS: a GET/attributes read of a delete marker answers 405 MethodNotAllowed with <Method> = the HTTP method of the request and <ResourceType> = DeleteMarker. Recorded behavior only. Emulator: both absent. Why: a client cannot distinguish "this version is a delete marker" from a generic method refusal. Fix: the delete-marker branch of the object read handler. Traces: s3-mbes-traj-plain-delete-then-version-purge#task#9 (GetObject), s3-ovm-traj-delete-marker-as-most-recent-entry#task#9 (GetObjectAttributes).

Cause 7 of 9: notification destination refusals omit ArgumentName1/ArgumentValue1. AWS: PutBucketNotificationConfiguration's destination-validation refusal carries <ArgumentName1> = the destination ARN from the request and <ArgumentValue1> = the fixed sentence for that destination class (for a queue, 'The destination queue does not exist'). Recorded behavior only. Emulator: both absent. Why: the caller cannot tell which of several destinations was unreachable. Fix: the notification destination-validation path. Traces: s3-put-rejects-nonexistent-queue-arn#task#5.

Cause 8 of 9: MetadataTooLarge refusals omit Size/MaxSizeAllowed. AWS: the metadata-size refusal carries <Size> = the computed header size of the request and <MaxSizeAllowed> = the service limit (2048 for user metadata). Recorded behavior only. Emulator: both absent (and the code/message also diverge — see the CopyObject bundle). Why: a client cannot compute how much metadata to drop. Fix: the metadata-size check in the object write/copy path. Traces: s3-sync-metadata-header-mistakes#task#5.

Cause 9 of 9: MalformedPolicy refusals omit Detail. AWS: PutBucketPolicy's MalformedPolicy carries <Detail> naming the offending action and statement, e.g. 'Action "<action>" in Statement "<sid>"', with action and sid taken from the submitted document. Recorded behavior only. Emulator: the refusal is not made at all (see the bucket-policy bundle), so Detail never appears. Fix: the PutBucketPolicy validator's error serializer. Traces: s3-cfoac-oac-grant-policy-document-mistakes#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator. Representative s3-cfg-bucket-tagging-refusals#task#2 (PutBucketTagging, expected 400 / actual 400): $.Error.TagKey missing (expected 'Environment', got None). Other causes, same shape: s3-cob-max-buckets-value-mistakes#task#5 -> $.Error.ArgumentName missing (expected 'max-buckets'), $.Error.ArgumentValue missing (expected '-1'); s3-ovm-traj-version-id-argument-mistakes#task#6 -> $.Error.Key missing, $.Error.VersionId missing; s3-lsd-invalid-bucket-name-reads-as-missing#task#5 -> $.Error.BucketName missing; s3-csvcols-part-number-reads#task#5 -> $.Error.PartNumberRequested missing (expected '2'), $.Error.ActualPartCount missing (expected '1'); s3-mbes-traj-plain-delete-then-version-purge#task#9 -> $.Error.Method missing (expected 'GET'), $.Error.ResourceType missing (expected 'DeleteMarker'); s3-put-rejects-nonexistent-queue-arn#task#5 -> $.Error.ArgumentName1/$.Error.ArgumentValue1 missing. In every case the HTTP status matched; only the envelope's context members were absent.

## Steps to Reproduce
1. Replay fixture s3-cfg-bucket-tagging-refusals and compare step task#2's response body against the recording. 2. Repeat for each fixture#step in trace_ids. 3. Each refusal should carry its error class's context members (TagKey/TagValue, ArgumentName/ArgumentValue, Key/VersionId, BucketName, PartNumberRequested/ActualPartCount, Method/ResourceType, ArgumentName1/ArgumentValue1, Size/MaxSizeAllowed) with values taken from the request or state.
