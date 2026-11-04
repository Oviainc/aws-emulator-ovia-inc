Issue-Source: trace-sweep-divergence
Issue-Trace: s3-mvb-copy-source-header-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-mvb-copy-source-header-mistakes s3-eescfn-overwrote-baseline-restore-from-version s3-sync-copy-source-mistakes s3-mvb-delete-before-copy-recovery s3-mvs3-delete-before-copy-version-recovery s3-sync-canned-acl-choices-on-enforced-bucket s3-sync-metadata-header-mistakes s3-txt-conditional-date-headers-and-precedence
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.Message
Issue-Key: s3|GetObject|4xx->4xx|$.Error.Condition
Issue-Related: 20261030T053639Z-s3-object-error-envelopes-omit-key-and-versionid.md
Issue-Related: 20261030T053639Z-s3-refusals-use-the-wrong-error-code-and.md
Issue-Related: 20261102T111455Z-s3-copyobject-source-resolution-and-refusal-taxonomy-diverge.md
Issue-Related: 20261103T054236Z-s3-object-read-request-validation-diverges-partnumber-sse.md
Issue-Commit: 490c1cbe5b0cc74feaea217ae97fc7bca73b7940

# s3 CopyObject header validation and conditional GetObject refusals diverge

> _Note: this issue was identified against the code base at commit `490c1cbe5b0cc74feaea217ae97fc7bca73b7940`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyObject

## Description
Seven net-new causes on the S3 object write/read path. They share one fault: request headers are validated by generic Smithy codec/constraint machinery and the resulting refusal is emitted verbatim, instead of being resolved against bucket/object state and reported with S3's own error code, message and contextual error members. An open issue, 20261102T111455Z-s3-copyobject-source-resolution-and-refusal-taxonomy-diverge.md, describes neighbouring CopyObject faults but does not carry the replay keys for any of the clusters below (the replay reports them unowned), so they are filed here; 20261030T053639Z-s3-multipart-range-and-precondition-error-envelopes-drop.md is likewise related to cause 7 but does not own it.

Cause 1 of 7: a percent-encoded x-amz-copy-source is refused by the modeled CopySource pattern instead of being decoded and resolved. AWS: the header value is URL-decoded into <source-bucket>/<source-key> and the request then fails on the resolved reference — 404 Code 'NoSuchBucket', Message 'The specified bucket does not exist' (fixed text) with <BucketName> set to the decoded source bucket name, or 404 Code 'NoSuchKey', Message 'The specified key does not exist.' (fixed text) with <Key> set to the decoded source key. Smithy: CopyObject.CopySource is httpHeader 'x-amz-copy-source' with @pattern ^\/?.+\/.+$; that pattern is satisfied by the DECODED value, the recording shows AWS resolving rather than rejecting it. Emulator: 400 Code 'InvalidArgument', Message "Value '<encoded header value>' at 'copySource' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\/?.+\\/.+$", and no BucketName/Key member. Why: a client copying any key whose name contains '/' or a space — which every SDK percent-encodes — is told its header is syntactically malformed rather than that the source bucket or key is missing, and cannot tell the two apart. Fix: percent-decode x-amz-copy-source before applying the pattern and before resolving the source in the CopyObject handler's source parser. Traces: s3-mvb-copy-source-header-mistakes#task#3 (expected NoSuchBucket), s3-mvb-copy-source-header-mistakes#task#4 (expected NoSuchKey).

Cause 2 of 7: a version-qualified copy source with a bad or stale versionId is refused with the wrong code and sentence. AWS: 400 Code 'InvalidRequest', Message 'Invalid Request' — both are fixed text, with no variable part. Recorded behavior only (CopyObject models no such error; the model's only bound error is ObjectNotInActiveTierError). Emulator: 400 Code 'InvalidArgument', Message 'Invalid version id specified'. Why: a client branching on the error code to decide between 'retry without a version' and 'fix the request' takes the wrong branch, and the code it sees is not one AWS ever returns for this case. Fix: the versionId branch of the CopyObject source resolver. Traces: s3-eescfn-overwrote-baseline-restore-from-version#task#7, s3-sync-copy-source-mistakes#task#6.

Cause 3 of 7: naming a delete marker by version id as the copy source answers MethodNotAllowed. AWS: 400 Code 'InvalidRequest', Message 'The source of a copy request may not specifically refer to a delete marker by version id.' — entirely fixed text. Recorded behavior only. Emulator: 405 Code 'MethodNotAllowed', Message 'The specified method is not allowed against this resource.' Why: both the HTTP status class and the explanation are wrong, so a client sees a routing/verb failure and never learns that the version id it supplied addresses a delete marker. Fix: the delete-marker branch of the CopyObject source resolver. Traces: s3-mvb-delete-before-copy-recovery#task#7.

Cause 4 of 7: an unparseable copy-source URI is blamed on the bucket. AWS: 400 Code 'InvalidArgument', Message 'Invalid copy source URI.' (fixed text) with <ArgumentName>x-amz-copy-source</ArgumentName> and <ArgumentValue>x-amz-copy-source</ArgumentValue>. Recorded behavior only. Emulator: 404 Code 'NoSuchBucket', Message 'The specified bucket does not exist', and neither ArgumentName nor ArgumentValue. Why: the caller is told a bucket is missing and goes off creating or re-resolving it, when the defect is the syntax of its own header. Fix: classify the parse failure in the CopyObject source parser before any state lookup. Traces: s3-sync-copy-source-mistakes#task#4.

Cause 5 of 7: an invalid canned ACL refusal leaks the Smithy enum constraint text and drops the argument members. AWS: 400 Code 'InvalidArgument' with an EMPTY <Message/>, <ArgumentName>x-amz-acl</ArgumentName> and <ArgumentValue> set to the rejected header value as sent. Smithy: CopyObject.ACL targets ObjectCannedACL, values [private, public-read, public-read-write, authenticated-read, aws-exec-read, bucket-owner-read, bucket-owner-full-control]. Emulator: 400 InvalidArgument whose Message is "Value '<value>' at 'aCL' failed to satisfy constraint: Member must satisfy enum value set: [private, public-read, ...]" — generated prose naming the member by its Rust-cased shape member ('aCL') and echoing the model's value list — with no ArgumentName/ArgumentValue. Why: clients and tooling that read ArgumentName/ArgumentValue to point at the offending header get nothing, and the message exposes emulator internals rather than S3's envelope. Fix: the CopyObject header validation path must map the enum rejection onto S3's InvalidArgument envelope (empty Message, ArgumentName/ArgumentValue). Traces: s3-sync-canned-acl-choices-on-enforced-bucket#task#6, s3-sync-copy-source-mistakes#task#5.

Cause 6 of 7: oversized user metadata is misclassified as a generic invalid parameter. AWS: 400 Code 'MetadataTooLarge', Message 'Your metadata headers exceed the maximum allowed metadata size' (fixed text), plus <Size> = the total size of the x-amz-meta-* headers in the request and <MaxSizeAllowed> = the limit S3 enforces (2048 for user metadata). Recorded behavior only. Emulator: 400 Code 'InvalidRequest', Message "A parameter or header in your request isn't valid. For details, see the description of this API operation.", with no Size or MaxSizeAllowed. Why: the caller cannot discover WHICH limit it breached nor by how much, so it cannot trim its metadata; code keyed on MetadataTooLarge never fires. Fix: a metadata-size check in CopyObject's header validation that emits the MetadataTooLarge envelope with both members computed from the request and the limit. Traces: s3-sync-metadata-header-mistakes#task#5.

Cause 7 of 7: GetObject parses the If-Unmodified-Since HTTP-date header with an RFC-3339 parser and fails the request. AWS: conditional headers are IMF-fixdate (HTTP-date); when the precondition does not hold the answer is 412 Code 'PreconditionFailed', Message 'At least one of the pre-conditions you specified did not hold' (fixed text) with <Condition> naming the header that failed, here 'If-Unmodified-Since'. Smithy: GetObject.IfUnmodifiedSince is httpHeader 'If-Unmodified-Since' of type timestamp, so the REST binding's timestamp format is http-date, not date-time. Emulator: 400 Code 'ValidationException', Message 'invalid timestamp `<the HTTP-date sent>`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times', and no Condition member. Why: every well-formed conditional GET sent by an SDK (which always formats these headers as HTTP-date) is rejected as a validation error instead of being evaluated, so conditional reads and precondition-based concurrency control cannot work at all. Fix: decode httpHeader-bound timestamps with the http-date format in the request deserializer, and evaluate the precondition in the GetObject handler to produce the PreconditionFailed envelope with Condition. Traces: s3-txt-conditional-date-headers-and-precedence#task#5, s3-txt-conditional-date-headers-and-precedence#task#10.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 77.8%, 705 mismatched steps).

Cause 1 — s3-mvb-copy-source-header-mistakes#task#3 (representative): status expected 404 got 400; Error.Code expected 'NoSuchBucket' got 'InvalidArgument'; Error.Message expected 'The specified bucket does not exist' got "Value '...%2Fsource%2Ffile1.txt' at 'copySource' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\/?.+\\/.+$"; Error.BucketName missing. #task#4: status expected 404 got 400; Code expected 'NoSuchKey' got 'InvalidArgument'; Message expected 'The specified key does not exist.'; Error.Key missing.

Cause 2 — s3-eescfn-overwrote-baseline-restore-from-version#task#7 (and #task#8): Code expected 'InvalidRequest' got 'InvalidArgument'; Message expected 'Invalid Request' got 'Invalid version id specified'. Same divergence at s3-sync-copy-source-mistakes#task#6.

Cause 3 — s3-mvb-delete-before-copy-recovery#task#7 (and s3-mvs3-delete-before-copy-version-recovery#task#7): status expected 400 got 405; Code expected 'InvalidRequest' got 'MethodNotAllowed'; Message expected 'The source of a copy request may not specifically refer to a delete marker by version id.' got 'The specified method is not allowed against this resource.'

Cause 4 — s3-sync-copy-source-mistakes#task#4: status expected 400 got 404; Code expected 'InvalidArgument' got 'NoSuchBucket'; Message expected 'Invalid copy source URI.' got 'The specified bucket does not exist'; Error.ArgumentName and Error.ArgumentValue (both 'x-amz-copy-source') missing.

Cause 5 — s3-sync-canned-acl-choices-on-enforced-bucket#task#6 (and s3-sync-copy-source-mistakes#task#5): Error.Message expected '' got "Value 'BucketOwnerFullControl' at 'aCL' failed to satisfy constraint: Member must satisfy enum value set: [private, public-read, public-read-write, authenticated-read, aws-exec-read, bucket-owner-read, bucket-owner-full-control]"; Error.ArgumentName expected 'x-amz-acl' missing; Error.ArgumentValue expected the rejected value missing.

Cause 6 — s3-sync-metadata-header-mistakes#task#5: Code expected 'MetadataTooLarge' got 'InvalidRequest'; Message expected 'Your metadata headers exceed the maximum allowed metadata size' got "A parameter or header in your request isn't valid. ..."; Error.Size and Error.MaxSizeAllowed missing (the recording shows the request's own total and the 2048 limit).

Cause 7 — s3-txt-conditional-date-headers-and-precedence#task#5 (and #task#10): status expected 412 got 400; Code expected 'PreconditionFailed' got 'ValidationException'; Message expected 'At least one of the pre-conditions you specified did not hold' got 'invalid timestamp `<HTTP-date>`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times'; Error.Condition expected 'If-Unmodified-Since' missing.

Per-run values (RequestId/HostId) also differ on these steps; that divergence is already owned by 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md and is not part of this issue.

## Steps to Reproduce
1. Replay fixture s3-mvb-copy-source-header-mistakes and compare steps task#3 and task#4 (cause 1).
2. Replay s3-eescfn-overwrote-baseline-restore-from-version step task#7 and s3-sync-copy-source-mistakes step task#6 (cause 2).
3. Replay s3-mvb-delete-before-copy-recovery step task#7 (cause 3).
4. Replay s3-sync-copy-source-mistakes step task#4 (cause 4).
5. Replay s3-sync-canned-acl-choices-on-enforced-bucket step task#6 (cause 5).
6. Replay s3-sync-metadata-header-mistakes step task#5 (cause 6).
7. Replay s3-txt-conditional-date-headers-and-precedence step task#5 (cause 7).
In each case compare the HTTP status, Error.Code, Error.Message and the contextual Error members (BucketName, Key, ArgumentName, ArgumentValue, Size, MaxSizeAllowed, Condition) against the recorded AWS response.
