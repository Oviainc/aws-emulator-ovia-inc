Issue-Source: trace-sweep-divergence
Issue-Trace: s3-object-tagging-acl-and-versioning
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-object-tagging-acl-and-versioning s3-object-versioning-and-lock s3-versioning-and-object-lock
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObjectTagging|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|GetObjectTagging|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|DeleteObjectTagging|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|PutObjectAcl|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|GetObjectAcl|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|PutObjectLegalHold|2xx->2xx|$headers.x-amz-version-id
Issue-Key: s3|PutObjectRetention|2xx->2xx|$headers.x-amz-version-id
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 object sub-resource writes omit the x-amz-version-id response header

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObjectTagging

## Description
Seven net-new root causes sharing one fault: on a versioning-enabled bucket, the object sub-resource operations do not report which version they acted on — the x-amz-version-id response header is absent from all of them. A fixer must close all seven.

AWS (common): each of these operations returns the version id of the object version whose tag set, ACL, legal hold or retention it read or wrote, in the x-amz-version-id response header. When the request named a versionId it is echoed; otherwise it is the current version's id. Smithy: PutObjectTaggingOutput.VersionId is bound to httpHeader x-amz-version-id; GetObjectTagging, DeleteObjectTagging, GetObjectAcl, PutObjectAcl, PutObjectLegalHold and PutObjectRetention bind the same member the same way on a versioned bucket.

Why (common): on a versioned bucket the caller cannot tell which version it just tagged, ACL'd, legal-held or retained, so it cannot write the value back to the same version or record the change against a stable identity. A read-modify-write of tags or an ACL silently targets whatever the current version happens to be at the next call. A related gap is noted as 'Cause 6' of the open behavioral issue 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md for GetObjectTagging only; it carries none of these wire replay keys and does not cover the other six operations.

Cause 1 of 7: PutObjectTagging omits x-amz-version-id.
Fix: the PutObjectTagging serializer and the version it resolved.
Traces: s3-object-tagging-acl-and-versioning#task#7.

Cause 2 of 7: GetObjectTagging omits x-amz-version-id.
Fix: the GetObjectTagging serializer.
Traces: s3-object-tagging-acl-and-versioning#task#8 (also #task#10).

Cause 3 of 7: DeleteObjectTagging omits x-amz-version-id on its 204.
Fix: the DeleteObjectTagging serializer (headers must still be emitted on a 204).
Traces: s3-object-tagging-acl-and-versioning#task#9.

Cause 4 of 7: PutObjectAcl omits x-amz-version-id.
Fix: the PutObjectAcl serializer.
Traces: s3-object-tagging-acl-and-versioning#task#11.

Cause 5 of 7: GetObjectAcl omits x-amz-version-id.
Fix: the GetObjectAcl serializer.
Traces: s3-object-tagging-acl-and-versioning#task#12.

Cause 6 of 7: PutObjectLegalHold omits x-amz-version-id.
Fix: the PutObjectLegalHold serializer.
Traces: s3-object-versioning-and-lock#task#9 (also s3-versioning-and-object-lock#task#8, #task#10).

Cause 7 of 7: PutObjectRetention omits x-amz-version-id.
Fix: the PutObjectRetention serializer.
Traces: s3-object-versioning-and-lock#task#11

## Evidence
All seven clusters are 'mismatch' with matching status; only the header diverged.
s3-object-tagging-acl-and-versioning#task#7 (PutObjectTagging, 200 -> 200): $headers.x-amz-version-id missing (expected the acted-on version id, got None)
s3-object-tagging-acl-and-versioning#task#8 (GetObjectTagging, 200 -> 200): $headers.x-amz-version-id missing
s3-object-tagging-acl-and-versioning#task#9 (DeleteObjectTagging, 204 -> 204): $headers.x-amz-version-id missing
s3-object-tagging-acl-and-versioning#task#11 (PutObjectAcl, 200 -> 200): $headers.x-amz-version-id missing
s3-object-tagging-acl-and-versioning#task#12 (GetObjectAcl, 200 -> 200): $headers.x-amz-version-id missing
s3-object-versioning-and-lock#task#9 (PutObjectLegalHold, 200 -> 200): $headers.x-amz-version-id missing
s3-object-versioning-and-lock#task#11 (PutObjectRetention, 200 -> 200): $headers.x-amz-version-id missing

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: enable versioning (and object lock where needed) on a bucket, PutObject a key, then call PutObjectTagging, GetObjectTagging, DeleteObjectTagging, PutObjectAcl, GetObjectAcl, PutObjectLegalHold and PutObjectRetention on that key. Each response must carry x-amz-version-id naming the version acted on.
