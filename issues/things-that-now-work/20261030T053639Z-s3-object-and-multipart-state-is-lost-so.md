Issue-Source: trace-sweep-divergence
Issue-Trace: s3-multipart-upload-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-multipart-upload-lifecycle s3w-checksum-mode-get-object-exact s3w-object-validation-error-envelopes s3w-delete-if-match-size-general-purpose s3w-object-lock-governance-delete s3-bucket-lifecycle-and-discovery s3-object-copy-and-rename
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListParts|2xx->2xx|$.ListPartsResult.Part
Issue-Key: s3|ListMultipartUploads|2xx->2xx|$.ListMultipartUploadsResult.NextUploadIdMarker
Issue-Key: s3|GetObject|2xx->4xx|$.__wire_length__
Issue-Key: s3|GetObject|2xx->4xx|$.__wire_sha256__
Issue-Key: s3|GetObject|2xx->4xx|$headers.content-length
Issue-Key: s3|GetObject|2xx->4xx|$headers.content-type
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|GetObject|2xx->4xx|status
Issue-Key: s3|AbortMultipartUpload|2xx->4xx|status
Issue-Key: s3|DeleteBucket|2xx->4xx|status
Issue-Key: s3|GetObject|2xx->4xx|$
Issue-Key: s3|GetObject|2xx->4xx|$headers.accept-ranges
Issue-Key: s3|GetObject|2xx->4xx|$headers.etag
Issue-Key: s3|GetObject|2xx->4xx|$headers.last-modified
Issue-Key: s3|DeleteObject|2xx->4xx|status
Issue-Key: s3|DeleteObjects|2xx->2xx|$.DeleteResult.Deleted[*]
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 object and multipart state is lost, so follow-up calls 404

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListParts

## Description
Ten net-new root causes sharing one fault: object and multipart state does not survive the operation that created it, so a later call in the same sequence answers 404/409/412 or returns an incomplete result. These are state-tracking defects, not serialization ones. A fixer must close all ten.

Why (common): each of these breaks a documented multi-step workflow — upload parts then list them, complete an upload then read the object, delete an object then delete its bucket. A client cannot distinguish 'the emulator lost it' from 'the service rejected my request', so it retries a sequence that can never converge, and teardown paths leave buckets undeletable.

Cause 1 of 10: ListParts returns only the first part of a multi-part upload.
AWS: <ListPartsResult> carries one <Part> element per uploaded part, each with PartNumber, LastModified, ETag and Size.
Emulator: a single Part object instead of the list, so the second uploaded part is absent.
Fix: the multipart upload's part registry and the ListParts handler that reads it.
Traces: s3-multipart-upload-lifecycle#task#6.

Cause 2 of 10: ListMultipartUploads omits NextUploadIdMarker on a truncated page.
AWS: when the listing is truncated, NextUploadIdMarker carries the upload id to resume from.
Emulator: absent, so the caller cannot page.
Fix: the ListMultipartUploads pagination state. A related open issue, 20261029T221139Z-s3-listmultipartuploads-sorts-same-key-uploads-by-upload.md, concerns ordering, not the marker, and carries no wire replay key for this step.
Traces: s3-multipart-upload-lifecycle#task#7.

Cause 3 of 10: GetObject 404s for the object a CompleteMultipartUpload just assembled.
AWS: 200 with the assembled bytes, content-type binary/octet-stream, content-length equal to the summed part sizes, and x-amz-server-side-encryption.
Emulator: 404 with an XML error body and content-length 229.
Fix: the CompleteMultipartUpload handler's materialization of the assembled object into object state.
Traces: s3-multipart-upload-lifecycle#task#9.

Cause 4 of 10: AbortMultipartUpload 404s for an upload that exists.
AWS: 204 No Content. (The open issue 20261029T211510Z-s3-refusal-fidelity-abortmultipartupload-is-not-a-no.md concerns abort idempotency and carries no wire replay key for this step.)
Emulator: 404.
Fix: the AbortMultipartUpload handler's upload lookup.
Traces: s3-multipart-upload-lifecycle#task#13.

Cause 5 of 10: DeleteBucket 409s for a bucket whose objects were all deleted.
AWS: 204 No Content once the bucket holds no objects, versions or delete markers.
Emulator: 409 BucketNotEmpty.
Fix: the bucket-emptiness accounting the DeleteBucket handler reads (object deletes must decrement it).
Traces: s3w-checksum-mode-get-object-exact#teardown#9 (also s3w-object-validation-error-envelopes#teardown#17).

Cause 6 of 10: GetObject 404s for an object that was written and not deleted.
AWS: 200 with the stored bytes, last-modified, etag, accept-ranges: bytes, x-amz-server-side-encryption and the stored content-type.
Emulator: 404 NoSuchKey with an XML body and content-type application/xml.
Fix: the object state behind the preceding write in this sequence — the object is lost before the read.
Traces: s3w-delete-if-match-size-general-purpose#task#5.

Cause 7 of 10: DeleteObject 412s where the precondition holds.
AWS: 204 No Content.
Emulator: 412 PreconditionFailed.
Fix: the DeleteObject conditional evaluator's comparison against current object state.
Traces: s3w-delete-if-match-size-general-purpose#task#6.

Cause 8 of 10: DeleteObject 404s on teardown for a governance-locked object version.
AWS: 204 No Content when the delete is permitted (with x-amz-bypass-governance-retention where required).
Emulator: 404.
Fix: the DeleteObject version lookup under object lock.
Traces: s3w-object-lock-governance-delete#teardown#10.

Cause 9 of 10: DeleteObjects omits DeleteMarkerVersionId from a Deleted entry that created a delete marker.
AWS: when a delete creates a marker, the <Deleted> entry carries DeleteMarker true and DeleteMarkerVersionId set to the marker's version id. Smithy: com.amazonaws.s3#DeletedObject models Key, VersionId, DeleteMarker and DeleteMarkerVersionId.
Emulator: the entry carries Key, VersionId and DeleteMarker but no DeleteMarkerVersionId.
Fix: the DeleteObjects handler's delete-marker bookkeeping and its result serializer.
Traces: s3-bucket-lifecycle-and-discovery#teardown#13.

Cause 10 of 10: DeleteObjects omits a requested key entirely from its result.
AWS: every key in the request appears in the result, under <Deleted> or <Error>.
Emulator: the third requested key (a part-copy destination) has no entry at all.
Fix: the DeleteObjects handler, which must report an outcome for every requested key, and the object state for the part-copy destination it cannot find.
Traces: s3-object-copy-and-rename#teardown#10

## Evidence
All ten clusters are 'mismatch'.
s3-multipart-upload-lifecycle#task#6 (ListParts): $.ListPartsResult.Part type_mismatch (expected a two-element list of parts, got the single part 1)
s3-multipart-upload-lifecycle#task#7 (ListMultipartUploads): $.ListMultipartUploadsResult.NextUploadIdMarker missing
s3-multipart-upload-lifecycle#task#9 (GetObject): status expected 200, got 404; $headers.content-length value_mismatch (expected the assembled size, got '229'); $headers.content-type value_mismatch (expected 'binary/octet-stream', got 'application/xml'); x-amz-server-side-encryption missing
s3-multipart-upload-lifecycle#task#13 (AbortMultipartUpload): status expected 204, got 404
s3w-checksum-mode-get-object-exact#teardown#9 (DeleteBucket): status expected 204, got 409
s3w-delete-if-match-size-general-purpose#task#5 (GetObject): status expected 200, got 404; body type_mismatch (expected the stored payload, got a NoSuchKey envelope); last-modified/etag/x-amz-server-side-encryption/accept-ranges missing; content-type expected 'text/plain', got 'application/xml'
s3w-delete-if-match-size-general-purpose#task#6 (DeleteObject): status expected 204, got 412
s3w-object-lock-governance-delete#teardown#10 (DeleteObject): status expected 204, got 404
s3-bucket-lifecycle-and-discovery#teardown#13 (DeleteObjects): $.DeleteResult.Deleted[1] expected DeleteMarker 'true' with DeleteMarkerVersionId, got the entry without DeleteMarkerVersionId
s3-object-copy-and-rename#teardown#10 (DeleteObjects): $.DeleteResult.Deleted[2] missing (expected an entry for the part-copy key, got None)

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: CreateMultipartUpload, UploadPart twice, ListParts (expect both parts), ListMultipartUploads with max-uploads=1 (expect NextUploadIdMarker), CompleteMultipartUpload then GetObject the key (expect 200 with the assembled bytes), AbortMultipartUpload on a live upload (expect 204); PutObject then DeleteObject then DeleteBucket (expect 204); DeleteObject with a matching If-Match (expect 204); on a versioned bucket DeleteObjects for several keys including one that creates a delete marker and one part-copy destination, and check every requested key has a result entry and that marker entries carry DeleteMarkerVersionId.
