Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bucket-lifecycle-notifications-replication
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-bucket-lifecycle-notifications-replication s3-bucket-metadata-and-metadata-table-configuration-management s3w-checksum-mode-get-object-exact s3w-object-validation-error-envelopes s3w-object-annotation-validation-errors s3-object-crud-and-metadata s3w-delete-if-match-size-general-purpose
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketReplication|4xx->2xx|$
Issue-Key: s3|PutBucketReplication|4xx->2xx|status
Issue-Key: s3|UpdateBucketMetadataJournalTableConfiguration|4xx->2xx|$
Issue-Key: s3|UpdateBucketMetadataJournalTableConfiguration|4xx->2xx|status
Issue-Key: s3|UpdateBucketMetadataInventoryTableConfiguration|4xx->2xx|$
Issue-Key: s3|UpdateBucketMetadataInventoryTableConfiguration|4xx->2xx|status
Issue-Key: s3|PutObject|4xx->2xx|$.Error
Issue-Key: s3|PutObject|4xx->2xx|status
Issue-Key: s3|ListObjectAnnotations|4xx->2xx|$.Error
Issue-Key: s3|ListObjectAnnotations|4xx->2xx|status
Issue-Key: s3|CreateBucket|4xx->2xx|$.Error
Issue-Key: s3|CreateBucket|4xx->2xx|status
Issue-Key: s3|PutObjectTagging|4xx->2xx|$.Error
Issue-Key: s3|PutObjectTagging|4xx->2xx|status
Issue-Key: s3|RenameObject|5xx->2xx|$.Error
Issue-Key: s3|RenameObject|5xx->2xx|status
Issue-Key: s3|DeleteObject|5xx->4xx|$.Error.Code
Issue-Key: s3|DeleteObject|5xx->4xx|$.Error.Header
Issue-Key: s3|DeleteObject|5xx->4xx|$.Error.Message
Issue-Key: s3|DeleteObject|5xx->4xx|status
Issue-Key: s3|DeleteObject|5xx->2xx|$
Issue-Key: s3|DeleteObject|5xx->2xx|status
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 answers success where AWS refuses the request

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketReplication

## Description
Ten net-new root causes sharing one fault: ten requests that AWS refuses are accepted with a 2xx and an empty or ordinary success body. Eight are missing input validations; two are unimplemented surfaces that AWS reports as NotImplemented but the emulator silently honours or ignores. A fixer must close all ten.

Why (common): a validation that does not fire is worse than a wrong message. The caller believes the configuration, the write or the conditional delete took effect, so the error path is never exercised and state silently diverges from what the client thinks it installed. For the NotImplemented cases a client feature-detects by calling the operation and reading 501; here it gets a success and concludes the feature works.

Cause 1 of 10: PutBucketReplication accepts a V2 replication configuration whose rule has no Priority.
AWS: 400 InvalidRequest, message 'Priority must be specified for this version of Cross Region Replication configuration schema. Please refer to S3 Developer Guide for more information.' (fixed text).
Emulator: 200 with an empty body.
Fix: the PutBucketReplication configuration validator.
Traces: s3-bucket-lifecycle-notifications-replication#task#11.

Cause 2 of 10: UpdateBucketMetadataJournalTableConfiguration accepts an update while the journal table is not ACTIVE.
AWS: 400 InvalidRequest, message "You can't perform this operation on a journal table that doesn't have an ACTIVE status. Wait for your journal table status to transition to ACTIVE, and then try again. If creation of the journal table fails, you must re-create the metadata configuration for your general purpose bucket." (fixed text).
Emulator: 200 with an empty body.
Fix: the UpdateBucketMetadataJournalTableConfiguration handler's table-status guard.
Traces: s3-bucket-metadata-and-metadata-table-configuration-management#task#4.

Cause 3 of 10: UpdateBucketMetadataInventoryTableConfiguration accepts an update while the journal table is still being created.
AWS: 400 InvalidRequest, message "You can't perform this operation while the journal table is being created. Wait for your journal table status to transition to ACTIVE, and then try again." (fixed text).
Emulator: 200 with an empty body.
Fix: the UpdateBucketMetadataInventoryTableConfiguration handler's table-status guard.
Traces: s3-bucket-metadata-and-metadata-table-configuration-management#task#5.

Cause 4 of 10: PutObject accepts a request carrying more than one x-amz-checksum-* header.
AWS: 400 InvalidRequest, message 'Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed.' (fixed text). Smithy: PutObject binds ChecksumCRC32/CRC32C/CRC64NVME/SHA1/SHA256/… each to its own x-amz-checksum-* header, and httpChecksum{requestAlgorithmMember: ChecksumAlgorithm} selects exactly one.
Emulator: 200, the object is written.
Fix: the PutObject checksum-header validation.
Traces: s3w-checksum-mode-get-object-exact#task#7 (also s3w-object-validation-error-envelopes#task#9).

Cause 5 of 10: ListObjectAnnotations accepts an invalid annotation prefix.
AWS: 400 InvalidPrefix, message 'The annotation prefix you have provided is invalid.' (fixed text).
Emulator: 200 with a normal listing.
Fix: the ListObjectAnnotations prefix validation.
Traces: s3w-object-annotation-validation-errors#task#6.

Cause 6 of 10: CreateBucket accepts a syntactically invalid bucket name.
AWS: 400 InvalidBucketName, message 'The specified bucket is not valid.' (fixed text), with <BucketName> echoing the rejected name. Uppercase letters and underscores are not legal in a general-purpose bucket name.
Emulator: 200 and the bucket is created.
Fix: the CreateBucket bucket-name validator. A related open issue, 20261029T211510Z-s3-accepts-requests-aws-refuses-bucket-policy-conditions.md, mentions bucket-name validation but carries no wire replay key for these steps.
Traces: s3w-object-validation-error-envelopes#task#5 (also s3w-object-validation-error-envelopes#task#6).

Cause 7 of 10: PutObjectTagging accepts a tag key prefixed with 'aws:'.
AWS: 400 InvalidTag, message 'Your TagKey cannot be prefixed with aws:' (fixed text), with <TagKey> echoing the rejected key.
Emulator: 200 and the tag set is stored.
Fix: the PutObjectTagging tag-key validation (the reserved aws: prefix).
Traces: s3w-object-validation-error-envelopes#task#11.

Cause 8 of 10: RenameObject is honoured instead of reported unimplemented.
AWS: on a general-purpose bucket the ?renameObject query parameter answers 501 NotImplemented, message 'A query parameter you provided implies functionality that is not implemented' (fixed text), with <QueryParameter>renameObject</QueryParameter>.
Emulator: 200.
Fix: the object-write dispatcher's handling of the renameObject query parameter on general-purpose buckets.
Traces: s3-object-crud-and-metadata#task#7.

Cause 9 of 10: DeleteObject treats x-amz-if-match-size as a supported precondition and fails it.
AWS: on a general-purpose bucket x-amz-if-match-size answers 501 NotImplemented, message 'A header you provided implies functionality that is not implemented' (fixed text), with <Header>x-amz-if-match-size</Header>. Smithy does bind DeleteObject.IfMatchSize to that header, but it is a directory-bucket feature; the general-purpose response is recorded behavior.
Emulator: 412 PreconditionFailed, Code 'PreconditionFailed', message 'At least one of the pre-conditions you specified did not hold'.
Fix: the DeleteObject conditional-header dispatcher, which must answer NotImplemented for x-amz-if-match-size on general-purpose buckets.
Traces: s3w-delete-if-match-size-general-purpose#task#3.

Cause 10 of 10: DeleteObject silently ignores x-amz-if-match-size when its value does match.
AWS: same 501 NotImplemented with <Header>x-amz-if-match-size</Header> regardless of the value.
Emulator: 204 and the object is deleted.
Fix: the same DeleteObject dispatcher, on the matching-value path.
Traces: s3w-delete-if-match-size-general-purpose#task#4

## Evidence
All ten clusters are 'mismatch' with a status divergence.
s3-bucket-lifecycle-notifications-replication#task#11 (PutBucketReplication): status expected 400, got 200; body empty_body (expected Error Code 'InvalidRequest' with the Priority sentence)
s3-bucket-metadata-and-metadata-table-configuration-management#task#4 (UpdateBucketMetadataJournalTableConfiguration): status expected 400, got 200; expected InvalidRequest with the non-ACTIVE journal-table sentence
s3-bucket-metadata-and-metadata-table-configuration-management#task#5 (UpdateBucketMetadataInventoryTableConfiguration): status expected 400, got 200; expected InvalidRequest with the journal-table-being-created sentence
s3w-checksum-mode-get-object-exact#task#7 (PutObject): status expected 400, got 200; expected InvalidRequest 'Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed.'
s3w-object-annotation-validation-errors#task#6 (ListObjectAnnotations): status expected 400, got 200; expected InvalidPrefix 'The annotation prefix you have provided is invalid.'
s3w-object-validation-error-envelopes#task#5 (CreateBucket): status expected 400, got 200; expected InvalidBucketName 'The specified bucket is not valid.' with BucketName echoing the invalid name
s3w-object-validation-error-envelopes#task#11 (PutObjectTagging): status expected 400, got 200; expected InvalidTag 'Your TagKey cannot be prefixed with aws:' with TagKey
s3-object-crud-and-metadata#task#7 (RenameObject): status expected 501, got 200; expected NotImplemented 'A query parameter you provided implies functionality that is not implemented' with QueryParameter 'renameObject'
s3w-delete-if-match-size-general-purpose#task#3 (DeleteObject): status expected 501, got 412; Code expected 'NotImplemented', got 'PreconditionFailed'; Message expected 'A header you provided implies functionality that is not implemented'; Header missing (expected 'x-amz-if-match-size')
s3w-delete-if-match-size-general-purpose#task#4 (DeleteObject): status expected 501, got 204; body empty_body (expected the NotImplemented envelope with Header 'x-amz-if-match-size')

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: PutBucketReplication with a V2 rule and no Priority; call the journal/inventory metadata-table update operations before the journal table is ACTIVE; PutObject with two different x-amz-checksum-* headers; ListObjectAnnotations with a malformed annotation prefix; CreateBucket with a name containing an uppercase letter and an underscore; PutObjectTagging with TagKey 'aws:reserved'; PUT /<bucket>/<key>?renameObject on a general-purpose bucket; DeleteObject with x-amz-if-match-size both matching and not matching on a general-purpose bucket. Each must be refused with the status, Code and message above instead of succeeding.
