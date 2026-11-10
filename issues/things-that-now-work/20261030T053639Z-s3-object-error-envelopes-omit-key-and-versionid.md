Issue-Source: trace-sweep-divergence
Issue-Trace: s3-copy-object-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-copy-object-lifecycle s3-object-copy-and-rename s3-object-core-crud-and-storage-management s3-object-crud-lifecycle-and-annotations s3w-bucket-policy-deny-and-owner-guard s3w-delete-if-match-size-general-purpose s3w-precondition-chain s3-object-restore-and-encryption s3-object-tagging-acl-and-torrent s3-object-tagging-acl-and-versioning s3-object-tagging-and-acl-management s3-object-versioning-and-delete-markers s3w-object-lock-governance-delete s3w-object-validation-error-envelopes s3w-put-object-checksum-mismatch-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.Key
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Key
Issue-Key: s3|RestoreObject|4xx->4xx|$.Error.Key
Issue-Key: s3|GetObjectTagging|4xx->4xx|$.Error.Key
Issue-Key: s3|PutObjectAcl|4xx->4xx|$.Error.Key
Issue-Key: s3|PutObjectTagging|4xx->4xx|$.Error.Key
Issue-Key: s3|GetObject|4xx->4xx|$headers.x-amz-delete-marker
Issue-Key: s3|GetObject|4xx->4xx|$headers.x-amz-version-id
Issue-Key: s3|GetObject|4xx->4xx|$.Error.VersionId
Issue-Key: s3|GetObject|4xx->4xx|$.Error.RequestId
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 object error envelopes omit Key and VersionId members

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Nine net-new root causes sharing one fault: every object-scoped NoSuchKey/NoSuchVersion refusal the emulator builds omits the <Key> member (and, where the request named a version, <VersionId>), and on a key hidden by a delete marker it also omits the x-amz-delete-marker / x-amz-version-id response headers. A fixer must close all nine.

AWS (common): an object-scoped error document is <Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message><Key>…</Key><RequestId/><HostId/></Error>. 'NoSuchKey' and 'The specified key does not exist.' are the fixed text; <Key> is a variable part copied verbatim from the request's key (decoded, including any slashes), and <VersionId> is present when the request carried versionId. Smithy: com.amazonaws.s3#NoSuchKey (404) models no members, so these envelope members are recorded behavior only; every recording here shows them.

Why (common): a client batching many keys through one code path, or an SDK that surfaces error['Key'], cannot tell which key failed. For the delete-marker case the missing headers are worse: 404 'key never existed' and 404 'latest version is a delete marker' are indistinguishable, so a caller cannot decide between creating the object and removing the marker.

Cause 1 of 9: CopyObject's 404 for a missing source key omits Key.
Emulator: $.Error.Key absent.
Fix: the CopyObject handler's source-not-found error construction.
Traces: s3-copy-object-lifecycle#task#5 (also s3-object-copy-and-rename#task#8).

Cause 2 of 9: GetObject's 404 NoSuchKey omits Key.
Emulator: $.Error.Key absent.
Fix: the GetObject not-found error construction.
Traces: s3-object-core-crud-and-storage-management#task#9 (also s3-object-core-crud-and-storage-management#task#11, s3-object-crud-lifecycle-and-annotations#task#6, s3w-bucket-policy-deny-and-owner-guard#task#8, s3w-delete-if-match-size-general-purpose#task#7, s3w-precondition-chain#task#15).

Cause 3 of 9: RestoreObject's 404 omits Key.
Emulator: $.Error.Key absent.
Fix: the RestoreObject handler's not-found error construction.
Traces: s3-object-restore-and-encryption#task#4.

Cause 4 of 9: GetObjectTagging's 404 omits Key.
Emulator: $.Error.Key absent.
Fix: the GetObjectTagging handler's not-found error construction.
Traces: s3-object-tagging-acl-and-torrent#task#13.

Cause 5 of 9: PutObjectAcl's 404 omits Key.
Emulator: $.Error.Key absent. Smithy does bind NoSuchKey (404) to PutObjectAcl.
Fix: the PutObjectAcl handler's not-found error construction.
Traces: s3-object-tagging-acl-and-versioning#task#13.

Cause 6 of 9: PutObjectTagging's 404 omits Key.
Emulator: $.Error.Key absent.
Fix: the PutObjectTagging handler's not-found error construction.
Traces: s3-object-tagging-and-acl-management#task#11.

Cause 7 of 9: GetObject on a key whose latest version is a delete marker omits Key and both delete-marker headers.
AWS: the 404 carries <Key>, plus the response headers x-amz-delete-marker: true and x-amz-version-id set to the marker's version id.
Emulator: $.Error.Key, x-amz-delete-marker and x-amz-version-id are all absent.
Why: a client cannot distinguish a deleted object from one that never existed, so it cannot choose between undeleting and creating. The open behavioral issue 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md describes the same header gap for HeadObject but carries no wire replay key for this GetObject step.
Fix: the GetObject/HeadObject not-found serializer, which must emit x-amz-delete-marker and x-amz-version-id when the head of the version chain is a delete marker.
Traces: s3-object-versioning-and-delete-markers#task#9.

Cause 8 of 9: GetObject with an explicit versionId that does not exist omits Key and VersionId.
Emulator: both $.Error.Key and $.Error.VersionId are absent.
Fix: the GetObject versioned-not-found error construction.
Traces: s3w-object-lock-governance-delete#task#9 (also s3w-object-validation-error-envelopes#task#12).

Cause 9 of 9: GetObject's 404 after a rejected checksum write omits Key.
Emulator: $.Error.Key absent. (The same cluster also shows a differing RequestId, which is a per-run value and not part of this defect.)
Fix: the GetObject not-found error construction.
Traces: s3w-put-object-checksum-mismatch-rejected#task#6

## Evidence
All nine clusters are 'mismatch' with matching status; only envelope members/headers diverged.
s3-copy-object-lifecycle#task#5 (CopyObject, 404 -> 404): $.Error.Key: missing (expected the requested source key, got None)
s3-object-core-crud-and-storage-management#task#9 (GetObject, 404 -> 404): $.Error.Key: missing (got None)
s3-object-restore-and-encryption#task#4 (RestoreObject, 404 -> 404): $.Error.Key: missing (got None)
s3-object-tagging-acl-and-torrent#task#13 (GetObjectTagging, 404 -> 404): $.Error.Key: missing (expected a key containing a '/', got None)
s3-object-tagging-acl-and-versioning#task#13 (PutObjectAcl, 404 -> 404): $.Error.Key: missing (got None)
s3-object-tagging-and-acl-management#task#11 (PutObjectTagging, 404 -> 404): $.Error.Key: missing (got None)
s3-object-versioning-and-delete-markers#task#9 (GetObject, 404 -> 404): $.Error.Key: missing; $headers.x-amz-delete-marker: missing (expected 'true'); $headers.x-amz-version-id: missing
s3w-object-lock-governance-delete#task#9 (GetObject, 404 -> 404): $.Error.Key: missing; $.Error.VersionId: missing
s3w-put-object-checksum-mismatch-rejected#task#6 (GetObject, 404 -> 404): $.Error.Key: missing (got None)

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand, against an existing bucket: GetObject, RestoreObject, GetObjectTagging, PutObjectTagging and PutObjectAcl on a key that does not exist; CopyObject whose x-amz-copy-source names a missing key; GetObject with a versionId that does not exist; on a versioned bucket, DeleteObject (creating a delete marker) then GetObject the key. Inspect each 404 body for <Key> (and <VersionId> where a version was requested), and the delete-marker case for the x-amz-delete-marker and x-amz-version-id headers.
