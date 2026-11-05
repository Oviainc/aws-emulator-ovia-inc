Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.head-object-on-delete-marker-rejected s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes s3.public-acl-on-an-owner-enforced-bucket-rejected s3.s3-oc-enforced-copy-and-multipart-accept-an-owner-only-canned-acl
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.head-object-on-delete-marker-rejected|run#25|AssertionError|assert headers.get("allow") == "DELETE", (
Issue-Key: s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes|run#44|AssertionError|assert marker_meta["HTTPHeaders"].get("allow") == "DELETE", (
Issue-Key: s3.public-acl-on-an-owner-enforced-bucket-rejected|_error#16|AssertionError|raise AssertionError(f"the call succeeded where AWS refuses it: {result!r}")
Issue-Key: s3|CopyObject|AccessControlListNotSupported
Issue-Commit: defa25c70e8324a13a1647a7aa41447c10b54fda

# s3: 3 net-new root causes — the delete-marker 405 carries no Allow/marker headers, and the BucketOwnerEnforced canned-ACL gate is wrong on both the multipart and copy routes

> _Note: this issue was identified against the code base at commit `defa25c70e8324a13a1647a7aa41447c10b54fda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
HeadObject

## Description
Three net-new root causes on the S3 object plane. Related open issues describe neighbouring faults but the replay reports none of these checks owned by them, so they are filed here and named for context: 20261103T190801Z-s3-getobjectacl-on-a-delete-marker-version-the.md (the same 405 envelope, but for GetObjectAcl), 20261103T040901Z-s3-10-net-new-root-causes-bucket-policy.md and 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md (delete-marker representation), and 20261027T001031Z-s3-7-net-new-root-causes-access-point.md.

Cause 1 of 3: the 405 MethodNotAllowed that refuses a delete-marker version carries none of its response headers.

AWS: addressing a delete marker by its own version id is refused on every read verb — HEAD and GET both answer HTTP 405 with Code 'MethodNotAllowed' and Message 'The specified method is not allowed against this resource.' (entirely fixed text, no variable part). Recorded behavior only: HeadObject's sole modeled error is NotFound and GetObject models only NoSuchKey/InvalidObjectState, so the model binds no error here. The refusal nonetheless carries the metadata that identifies what was addressed: 'allow: DELETE' (the one verb a marker accepts), 'x-amz-delete-marker: true', 'x-amz-version-id' echoing the addressed marker version id, and 'last-modified' for the marker. Smithy: HeadObject/GetObject output bind DeleteMarker to httpHeader 'x-amz-delete-marker', VersionId to 'x-amz-version-id' and LastModified to 'Last-Modified'; the variable parts of those headers are the marker's version id and its write time, taken from state, while 'allow' is the fixed string 'DELETE'. The unversioned form is different and must stay different: HEAD/GET on a delete-marked KEY (no versionId) is 404 carrying 'x-amz-delete-marker: true' and 'x-amz-version-id' but NO allow header.

Emulator: the status and error code are right, but the 405 response headers are only {content-type, x-amz-request-id, x-amz-id-2, content-length, date} — 'allow' is absent (observed None), and 'x-amz-delete-marker', 'x-amz-version-id' and 'last-modified' are absent too.

Why: those headers are the only way a client distinguishes "this version id names a delete marker, which exists and can only be removed" from "this version id does not exist" and from "this key never existed" — all three are bare 4xx otherwise. Versioning tools that resurrect an object by deleting its marker cannot discover the marker's version id from the refusal, and clients that read the Allow header to choose a verb are told nothing.

Fix: the refusal path for a version-addressed delete marker in the HeadObject and GetObject handlers (and the serializer that emits it) must attach allow: DELETE, x-amz-delete-marker: true, the addressed x-amz-version-id and the marker's last-modified; the unversioned 404 path must keep carrying the marker/version headers and must keep omitting allow.

Traces: s3.head-object-on-delete-marker-rejected, s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes.

Cause 2 of 3: CreateMultipartUpload accepts ACL=public-read on a BucketOwnerEnforced bucket.

AWS: with ObjectOwnership BucketOwnerEnforced (the default for a bucket created without OwnershipControls) ACLs are disabled on every object-creating route, not only PutObject. A CreateMultipartUpload naming a non-owner canned ACL is refused with Code 'AccessControlListNotSupported' and Message 'The bucket does not allow ACLs' — fixed text with no variable part — and no upload is opened, so ListMultipartUploads shows nothing for that key. Smithy: CreateMultipartUpload.ACL is httpHeader 'x-amz-acl' targeting ObjectCannedACL; CreateMultipartUpload binds no modeled errors, so the refusal is recorded behavior only. The check holds BlockPublicAcls OFF (IgnorePublicAcls/BlockPublicPolicy/RestrictPublicBuckets ON) precisely so the refusal cannot be credited to Block Public Access — ObjectOwnership is the only guard being tested.

Emulator: the call succeeds with HTTP 200 and returns a real UploadId, so a public-read multipart upload is opened on a bucket where ACLs are disabled. The sibling CopyObject-with-public-read case in the same check IS refused correctly, so the gate exists but is not wired into the CreateMultipartUpload route.

Why: the request that AWS refuses is admitted, and the object it goes on to create carries an ACL the bucket's ownership setting forbids. A client that relies on BucketOwnerEnforced to make public grants impossible gets a silently public object, and code branching on AccessControlListNotSupported never fires.

Fix: apply the ObjectOwnership/canned-ACL gate in the CreateMultipartUpload handler's validation, before any upload state is created, emitting the AccessControlListNotSupported envelope.

Traces: s3.public-acl-on-an-owner-enforced-bucket-rejected.

Cause 3 of 3: CopyObject refuses the owner-only canned ACL that a BucketOwnerEnforced bucket still accepts.

AWS: BucketOwnerEnforced disables ACLs but exempts the owner-only canned ACLs, which grant nothing to anyone but the bucket owner — recorded behavior: CopyObject and CreateMultipartUpload with ACL=bucket-owner-read succeed on an owner-enforced bucket (HTTP 200) and create the object; only ACLs that grant a third party (public-read and the like) are refused AccessControlListNotSupported. Smithy: CopyObject.ACL / CreateMultipartUpload.ACL are httpHeader 'x-amz-acl' targeting ObjectCannedACL, whose values include bucket-owner-read and bucket-owner-full-control; CopyObject's only modeled error is ObjectNotInActiveTierError, so the accept/refuse split is recorded behavior only.

Emulator: CopyObject with ACL=bucket-owner-read is refused — HTTP 400, Code 'AccessControlListNotSupported', Message 'The bucket does not allow ACLs' — so the copy never happens and the subsequent CreateMultipartUpload/UploadPart/CompleteMultipartUpload leg of the same flow is never reached. The emulator's gate refuses every canned ACL rather than only the non-owner ones.

Why: taken with cause 2 the gate is wrong in both directions — it admits the public ACL AWS refuses and refuses the owner-only ACL AWS admits. SDK helpers and sync/copy tooling that always stamp bucket-owner-full-control or bucket-owner-read onto cross-account copies are blocked outright on the emulator.

Fix: the ObjectOwnership canned-ACL gate shared by the CopyObject and CreateMultipartUpload handlers must classify the canned ACL: owner-only values pass, third-party-granting values are refused AccessControlListNotSupported.

Traces: s3.s3-oc-enforced-copy-and-multipart-accept-an-owner-only-canned-acl.

## Evidence
Behavioral replay against the emulator built from this checkout.

Cause 1 — s3.head-object-on-delete-marker-rejected (mismatch):
  assert headers.get("allow") == "DELETE"
  AssertionError: the 405 must advertise the one permitted method with allow: DELETE, got None — that header is how a client learns the marker can only be removed, never read
Status 405 and Code MethodNotAllowed were correct; the header set was not.

Cause 1 — s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes (mismatch):
  assert marker_meta["HTTPHeaders"].get("allow") == "DELETE"
  AssertionError: the 405 must advertise Allow: DELETE — the marker's only legal verb; headers were {'content-type': 'application/xml', 'x-amz-request-id': ..., 'x-amz-id-2': ..., 'content-length': '302', 'date': ...}
Expected additionally on that 405: x-amz-delete-marker: true, x-amz-version-id = the addressed marker version id, last-modified. All absent.

Cause 2 — s3.public-acl-on-an-owner-enforced-bucket-rejected (mismatch):
  AssertionError: the call succeeded where AWS refuses it: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Bucket': <bucket>, 'Key': 'uploaded/public.bin', 'UploadId': <id>}
Expected: Code 'AccessControlListNotSupported', Message 'The bucket does not allow ACLs', and no open upload for that key. GetBucketOwnershipControls confirmed Rules == [{'ObjectOwnership': 'BucketOwnerEnforced'}] and PutPublicAccessBlock had BlockPublicAcls=False, so Block Public Access is not the guard under test. The CopyObject-with-public-read leg of the same check passed, showing the gate is route-specific.

Cause 3 — s3.s3-oc-enforced-copy-and-multipart-accept-an-owner-only-canned-acl (error):
  botocore.exceptions.ClientError: An error occurred (AccessControlListNotSupported) when calling the CopyObject operation: The bucket does not allow ACLs
Expected: CopyObject(ACL='bucket-owner-read') succeeds, then CreateMultipartUpload(ACL='bucket-owner-read') + UploadPart + CompleteMultipartUpload succeed, and the listing is ['copied', 'multi', 'src'].

## Steps to Reproduce
Cause 1:
1. Create a bucket, PutBucketVersioning Status=Enabled, PutObject a key, then DeleteObject the key without a VersionId; keep the returned delete-marker VersionId.
2. HeadObject(Bucket, Key, VersionId=<marker id>): AWS answers 405 with headers allow: DELETE, x-amz-delete-marker: true, x-amz-version-id: <marker id>, last-modified. The emulator answers 405 with none of them.
3. GetObject with the same VersionId: AWS answers 405 Code MethodNotAllowed, Message 'The specified method is not allowed against this resource.', again with allow: DELETE.
4. HeadObject(Bucket, Key) with no VersionId must stay 404 with x-amz-delete-marker/x-amz-version-id and NO allow header.

Cause 2:
1. CreateBucket with no OwnershipControls (defaults to BucketOwnerEnforced); PutPublicAccessBlock with BlockPublicAcls=False and the other three True.
2. CreateMultipartUpload(Bucket, Key, ACL='public-read'): AWS refuses AccessControlListNotSupported / 'The bucket does not allow ACLs' and opens no upload. The emulator returns 200 with an UploadId.

Cause 3:
1. On the same owner-enforced bucket, PutObject a source key.
2. CopyObject(ACL='bucket-owner-read'): AWS succeeds. The emulator refuses AccessControlListNotSupported.
3. CreateMultipartUpload(ACL='bucket-owner-read') + UploadPart + CompleteMultipartUpload: AWS succeeds.

Re-replay s3.head-object-on-delete-marker-rejected, s3.s3-delete-marker-hides-the-object-while-the-version-still-returns-its-bytes, s3.public-acl-on-an-owner-enforced-bucket-rejected and s3.s3-oc-enforced-copy-and-multipart-accept-an-owner-only-canned-acl to confirm.
