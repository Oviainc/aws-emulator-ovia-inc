Issue-Source: trace-sweep-divergence
Issue-Trace: s3w-multipart-complete-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3w-multipart-complete-errors s3w-multipart-entity-too-small s3-object-crud-and-metadata s3-object-tagging-acl-and-torrent s3w-range-requests s3w-precondition-chain
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CompleteMultipartUpload|4xx->4xx|$.Error.UploadId
Issue-Key: s3|CompleteMultipartUpload|4xx->4xx|$.Error.ETag
Issue-Key: s3|CompleteMultipartUpload|4xx->4xx|$.Error.PartNumber
Issue-Key: s3|CompleteMultipartUpload|4xx->4xx|$.Error.MinSizeAllowed
Issue-Key: s3|CompleteMultipartUpload|4xx->4xx|$.Error.ProposedSize
Issue-Key: s3|GetObjectTorrent|4xx->4xx|$.Error.Method
Issue-Key: s3|GetObjectTorrent|4xx->4xx|$.Error.ResourceType
Issue-Key: s3|GetObject|4xx->4xx|$.Error.ActualObjectSize
Issue-Key: s3|GetObject|4xx->4xx|$.Error.RangeRequested
Issue-Key: s3|PutObject|4xx->4xx|$.Error.Condition
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Condition
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.Condition
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.Message
Issue-Key: s3|DeleteObject|4xx->4xx|$.Error.Condition
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 multipart, range and precondition error envelopes drop their detail members

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteMultipartUpload

## Description
Ten net-new root causes sharing one fault: the detail members AWS packs into its multipart, range and precondition refusals are not serialized at all. The status and Code are right; the members that make the refusal actionable are missing. A fixer must close all ten.

AWS (common): an S3 error document carries, besides Code/Message/RequestId/HostId, the members specific to that error class, each filled from the request or from server state: NoSuchUpload carries <UploadId>; InvalidPart carries <UploadId>, <PartNumber> and <ETag>; EntityTooSmall carries <ProposedSize>, <MinSizeAllowed>, <PartNumber> and <ETag>; MethodNotAllowed for ?torrent carries <Method> and <ResourceType>; InvalidRange carries <RangeRequested> and <ActualObjectSize>; PreconditionFailed carries <Condition> naming the header that did not hold. Smithy models none of these error shapes' members (NoSuchUpload declares no members; InvalidPart/EntityTooSmall are not in the model), so this is recorded behavior only.

Why (common): these members are the machine-readable part of the refusal. Without <PartNumber>/<ETag> a client cannot re-upload the offending part; without <ProposedSize>/<MinSizeAllowed> it cannot resize; without <RangeRequested>/<ActualObjectSize> it cannot clamp the next range; without <Condition> it cannot tell which of several conditional headers (If-Match vs If-None-Match vs x-amz-copy-source-if-match) failed, which is exactly what a conditional-write retry loop keys on.

Cause 1 of 10: CompleteMultipartUpload's 404 NoSuchUpload omits UploadId.
Emulator: $.Error.UploadId absent.
Fix: the CompleteMultipartUpload handler's unknown-upload error construction.
Traces: s3w-multipart-complete-errors#task#2.

Cause 2 of 10: CompleteMultipartUpload's 400 InvalidPart omits UploadId, PartNumber and ETag.
Emulator: all three members absent.
Fix: the CompleteMultipartUpload part-validation error construction.
Traces: s3w-multipart-complete-errors#task#6.

Cause 3 of 10: CompleteMultipartUpload's 400 for a part-list problem omits UploadId.
Emulator: $.Error.UploadId absent.
Fix: the CompleteMultipartUpload part-list validation error construction.
Traces: s3w-multipart-complete-errors#task#7.

Cause 4 of 10: CompleteMultipartUpload's 400 EntityTooSmall omits ProposedSize, MinSizeAllowed, PartNumber and ETag.
AWS: MinSizeAllowed is the service's 5 MiB part floor; ProposedSize is the offending part's actual size; PartNumber and ETag identify the part.
Emulator: all four members absent.
Fix: the CompleteMultipartUpload minimum-part-size validation error construction.
Traces: s3w-multipart-entity-too-small#task#5.

Cause 5 of 10: GetObjectTorrent's 405 omits Method and ResourceType.
AWS: the 405 envelope carries <Method>GET</Method> and <ResourceType>TORRENT</ResourceType>.
Emulator: both absent.
Fix: the GetObjectTorrent handler's method-not-allowed error construction.
Traces: s3-object-crud-and-metadata#task#8 (also s3-object-tagging-acl-and-torrent#task#11).

Cause 6 of 10: GetObject's 416 InvalidRange omits RangeRequested and ActualObjectSize.
AWS: <RangeRequested> echoes the Range header verbatim; <ActualObjectSize> is the object's size in bytes.
Emulator: both absent.
Fix: the GetObject range evaluator's error construction.
Traces: s3w-range-requests#task#8.

Cause 7 of 10: PutObject's 412 PreconditionFailed omits Condition for If-None-Match.
AWS: <Condition>If-None-Match</Condition> names the header that failed.
Emulator: $.Error.Condition absent.
Fix: the PutObject conditional-write evaluator's error construction.
Traces: s3w-precondition-chain#task#3 (also s3w-precondition-chain#task#6).

Cause 8 of 10: GetObject's 412 omits Condition for If-Match.
Emulator: $.Error.Condition absent.
Fix: the GetObject conditional-read evaluator's error construction.
Traces: s3w-precondition-chain#task#9.

Cause 9 of 10: CopyObject's 412 omits Condition and its Message carries an emulator-added suffix.
AWS: Message is exactly 'At least one of the pre-conditions you specified did not hold' and the failing header is reported in <Condition>x-amz-copy-source-If-Match</Condition>.
Emulator: Message is 'At least one of the pre-conditions you specified did not hold: x-amz-copy-source-if-match' (the header name appended to the fixed text) and <Condition> is absent.
Fix: the CopyObject conditional-copy evaluator's error construction and message.
Traces: s3w-precondition-chain#task#10.

Cause 10 of 10: DeleteObject's 412 omits Condition for If-Match.
Emulator: $.Error.Condition absent.
Fix: the DeleteObject conditional-delete evaluator's error construction.
Traces: s3w-precondition-chain#task#12

## Evidence
All ten clusters are 'mismatch' with matching status.
s3w-multipart-complete-errors#task#2 (CompleteMultipartUpload, 404 -> 404): $.Error.UploadId: missing (got None)
s3w-multipart-complete-errors#task#6 (CompleteMultipartUpload, 400 -> 400): $.Error.UploadId / $.Error.PartNumber / $.Error.ETag all missing
s3w-multipart-complete-errors#task#7 (CompleteMultipartUpload, 400 -> 400): $.Error.UploadId: missing
s3w-multipart-entity-too-small#task#5 (CompleteMultipartUpload, 400 -> 400): $.Error.ProposedSize missing (expected the part's size), $.Error.MinSizeAllowed missing (expected '5242880'), $.Error.PartNumber missing, $.Error.ETag missing
s3-object-crud-and-metadata#task#8 (GetObjectTorrent, 405 -> 405): $.Error.Method missing (expected 'GET'), $.Error.ResourceType missing (expected 'TORRENT')
s3w-range-requests#task#8 (GetObject, 416 -> 416): $.Error.RangeRequested missing (expected the Range header value), $.Error.ActualObjectSize missing (expected the object's byte size)
s3w-precondition-chain#task#3 (PutObject, 412 -> 412): $.Error.Condition missing (expected 'If-None-Match')
s3w-precondition-chain#task#9 (GetObject, 412 -> 412): $.Error.Condition missing (expected 'If-Match')
s3w-precondition-chain#task#10 (CopyObject, 412 -> 412): $.Error.Message value_mismatch (expected 'At least one of the pre-conditions you specified did not hold', got '... did not hold: x-amz-copy-source-if-match'); $.Error.Condition missing (expected 'x-amz-copy-source-If-Match')
s3w-precondition-chain#task#12 (DeleteObject, 412 -> 412): $.Error.Condition missing (expected 'If-Match')

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: CompleteMultipartUpload with an unknown uploadId, with a bogus part ETag, and after uploading a single part smaller than 5 MiB; GET /<bucket>/<key>?torrent; GetObject with a Range beyond the object's size; PutObject with If-None-Match that does not hold, GetObject and DeleteObject with a non-matching If-Match, and CopyObject with a non-matching x-amz-copy-source-if-match. Inspect each error body for the class-specific members listed above.
