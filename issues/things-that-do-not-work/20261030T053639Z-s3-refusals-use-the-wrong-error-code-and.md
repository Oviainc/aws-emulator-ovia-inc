Issue-Source: trace-sweep-divergence
Issue-Trace: s3-metadata-configuration-invalid-record-expiration-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-metadata-configuration-invalid-record-expiration-error s3-object-tagging-and-acl-management s3w-copy-object-metadata-directive s3w-list-objects-v2-continuation-pagination s3w-list-objects-v2-delimiter-rollup s3w-object-annotation-validation-errors s3w-object-validation-error-envelopes s3-bulk-object-delete-and-bucket-service-settings
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CreateBucketMetadataConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|CreateBucketMetadataConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|PutObjectAcl|4xx->4xx|$.Error.Message
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.RequestId
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|CopyObject|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListObjectsV2|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|ListObjectsV2|4xx->4xx|$.Error.RequestId
Issue-Key: s3|ListObjectsV2|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListObjectsV2|4xx->4xx|$.Error.Message
Issue-Key: s3|ListObjectAnnotations|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|ListObjectAnnotations|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListObjectAnnotations|4xx->4xx|$.Error.Message
Issue-Key: s3|PutObject|4xx->4xx|$.Error.Code
Issue-Key: s3|PutObject|4xx->4xx|$.Error.MaxSizeAllowed
Issue-Key: s3|PutObject|4xx->4xx|$.Error.Message
Issue-Key: s3|PutObject|4xx->4xx|$.Error.Size
Issue-Key: s3|PutObject|4xx->4xx|$.Error.StorageClassRequested
Issue-Key: s3|DeleteObjects|4xx->4xx|$.Error.Message
Issue-Related: 20261030T053639Z-s3-multipart-range-and-precondition-error-envelopes-drop.md
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 refusals use the wrong error Code and emulator-worded messages

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucketMetadataConfiguration

## Description
Ten net-new root causes sharing one fault: where AWS has a specific error Code and a fixed message for a rejected request, the emulator falls back on a generic Smithy constraint-violation refusal — a generic Code (InvalidArgument / InvalidRequest), a generated 'Value X at Y failed to satisfy constraint…' or 'A parameter or header in your request isn't valid.' message, and no argument-naming members. The status is right; the Code, the message and the members are not. A fixer must close all ten.

AWS (common): an HTTP status, an error Code and an error message are all requirements. AWS's message for each class below has fixed text plus variable parts taken from the request (the argument name, its value, the offending size, the limit). AWS also names the offending input in <ArgumentName>/<ArgumentValue> (or the class-specific equivalent). Smithy does not model these error shapes or their members for S3, so the Codes, messages and members here are recorded behavior.

Why (common): a client branches on Code, not on prose. Reporting InvalidArgument where AWS reports InvalidRequest, KeyTooLongError or InvalidStorageClass sends the client down the wrong branch; and a Smithy-generated constraint sentence leaks the emulator's internal member names ('maxAnnotationResults', 'encodingType', 'storageClass') instead of the wire names AWS uses, so no message-matching or user-facing report built against AWS works here.

Cause 1 of 10: CreateBucketMetadataConfiguration rejects an out-of-range RecordExpiration.Days with the wrong Code and message.
AWS: 400 InvalidRequest, message 'When RecordExpiration is set to ENABLED, the Days value must be specified. This value must be a whole number from 7 to 2147483647. Specify a valid Days value, and then try again.' (entirely fixed text).
Emulator: Code 'InvalidArgument', message 'RecordExpiration.Days must be a whole number from 7 to 2147483647, got <n>'.
Fix: the CreateBucketMetadataConfiguration RecordExpiration validation's error class and message.
Traces: s3-metadata-configuration-invalid-record-expiration-error#task#2.

Cause 2 of 10: PutObjectAcl rejects a canned ACL combined with grant headers with a generic message.
AWS: 400, message 'Specifying both Canned ACLs and Header Grants is not allowed' (fixed text).
Emulator: "A parameter or header in your request isn't valid. For details, see the description of this API operation."
Fix: the PutObjectAcl ACL/grant mutual-exclusion validation's message.
Traces: s3-object-tagging-and-acl-management#task#12.

Cause 3 of 10: CopyObject rejects a self-copy with a generic message.
AWS: 400, message "This copy request is illegal because it is trying to copy an object to itself without changing the object's metadata, storage class, website redirect location or encryption attributes." (fixed text).
Emulator: "A parameter or header in your request isn't valid. …"
Fix: the CopyObject self-copy validation's message.
Traces: s3w-copy-object-metadata-directive#task#7.

Cause 4 of 10: CopyObject's refusal of an unsupported x-amz-metadata-directive omits ArgumentName/ArgumentValue.
AWS: the 400 carries <ArgumentName>x-amz-metadata-directive</ArgumentName> and <ArgumentValue> set to the rejected directive.
Emulator: both members absent.
Fix: the CopyObject metadata-directive validation's error construction.
Traces: s3w-copy-object-metadata-directive#task#8.

Cause 5 of 10: ListObjectsV2's refusal of a malformed continuation-token omits ArgumentName.
AWS: the 400 carries <ArgumentName>continuation-token</ArgumentName>.
Emulator: absent.
Fix: the ListObjectsV2 continuation-token validation's error construction.
Traces: s3w-list-objects-v2-continuation-pagination#task#11.

Cause 6 of 10: ListObjectsV2's refusal of an unknown encoding-type uses a generated constraint message and omits ArgumentName/ArgumentValue.
AWS: 400, message 'Invalid Encoding Method specified in Request' (fixed text), with <ArgumentName>encoding-type</ArgumentName> and <ArgumentValue> set to the rejected value.
Emulator: "Value '<v>' at 'encodingType' failed to satisfy constraint: Member must satisfy enum value set: [url]" and no argument members. Smithy: ListObjectsV2.EncodingType is bound to the query parameter 'encoding-type', so even the member name in the emulator's message is the wrong one.
Fix: the ListObjectsV2 encoding-type validation's error class, message and members.
Traces: s3w-list-objects-v2-delimiter-rollup#task#8.

Cause 7 of 10: ListObjectAnnotations' refusal of an out-of-range max-annotation-results uses a generated constraint message and omits the argument members.
AWS: 400, message 'Argument max-annotation-results must be an integer between 1 and 1000' (fixed text with the limits), plus <ArgumentName>max-annotation-results</ArgumentName> and <ArgumentValue> set to the rejected value.
Emulator: "Value '<v>' at 'maxAnnotationResults' failed to satisfy constraint: Member must have value greater than or equal to 1" and no argument members.
Fix: the ListObjectAnnotations max-annotation-results validation's message and members.
Traces: s3w-object-annotation-validation-errors#task#7 (also s3w-object-annotation-validation-errors#task#8).

Cause 8 of 10: PutObject with an over-long key answers the wrong Code and loses the size members.
AWS: 400 KeyTooLongError, message 'Your key is too long' (fixed text), with <Size> set to the submitted key length and <MaxSizeAllowed> set to the 1024-byte limit.
Emulator: Code 'InvalidRequest', message "A parameter or header in your request isn't valid. …", no Size/MaxSizeAllowed.
Fix: the PutObject key-length validation's error class, message and members.
Traces: s3w-object-validation-error-envelopes#task#7.

Cause 9 of 10: PutObject with an unknown storage class answers the wrong Code and a generated enum message.
AWS: 400 InvalidStorageClass, message 'The storage class you specified is not valid' (fixed text), with <StorageClassRequested> set to the rejected value.
Emulator: Code 'InvalidArgument' and "Value '<v>' at 'storageClass' failed to satisfy constraint: Member must satisfy enum value set: [STANDARD, REDUCED_REDUNDANCY, …]", no StorageClassRequested. Smithy: PutObject.StorageClass is bound to the x-amz-storage-class header, which is what AWS's refusal talks about.
Fix: the PutObject storage-class validation's error class, message and member.
Traces: s3w-object-validation-error-envelopes#task#8.

Cause 10 of 10: DeleteObjects' MalformedXML message carries an emulator-added explanation.
AWS: 400, message exactly 'The XML you provided was not well-formed or did not validate against our published schema' — the object-count limit is not part of the message.
Emulator: appends ': a Delete request carries from 1 to 1000 Object elements, and this one carries <n>'.
Fix: the DeleteObjects request validation's message.
Traces: s3-bulk-object-delete-and-bucket-service-settings#task#18

## Evidence
All ten clusters are 'mismatch'.
s3-metadata-configuration-invalid-record-expiration-error#task#2 (CreateBucketMetadataConfiguration, 400 -> 400): $.Error.Code value_mismatch (expected 'InvalidRequest', got 'InvalidArgument'); $.Error.Message value_mismatch (expected 'When RecordExpiration is set to ENABLED, the Days value must be specified. This value must be a whole number from 7 to 2147483647. Specify a valid Days value, and then try again.', got 'RecordExpiration.Days must be a whole number from 7 to 2147483647, got 1')
s3-object-tagging-and-acl-management#task#12 (PutObjectAcl, 400 -> 400): $.Error.Message value_mismatch (expected 'Specifying both Canned ACLs and Header Grants is not allowed')
s3w-copy-object-metadata-directive#task#7 (CopyObject, 400 -> 400): $.Error.Message value_mismatch (expected the self-copy sentence)
s3w-copy-object-metadata-directive#task#8 (CopyObject, 400 -> 400): $.Error.ArgumentName missing (expected 'x-amz-metadata-directive'); $.Error.ArgumentValue missing
s3w-list-objects-v2-continuation-pagination#task#11 (ListObjectsV2, 400 -> 400): $.Error.ArgumentName missing (expected 'continuation-token')
s3w-list-objects-v2-delimiter-rollup#task#8 (ListObjectsV2, 400 -> 400): $.Error.Message value_mismatch (expected 'Invalid Encoding Method specified in Request', got "Value 'bogus' at 'encodingType' failed to satisfy constraint: Member must satisfy enum value set: [url]"); ArgumentName/ArgumentValue missing
s3w-object-annotation-validation-errors#task#7 (ListObjectAnnotations, 400 -> 400): $.Error.Message value_mismatch (expected 'Argument max-annotation-results must be an integer between 1 and 1000'); ArgumentName/ArgumentValue missing
s3w-object-validation-error-envelopes#task#7 (PutObject, 400 -> 400): $.Error.Code value_mismatch (expected 'KeyTooLongError', got 'InvalidRequest'); Message value_mismatch (expected 'Your key is too long'); Size and MaxSizeAllowed missing
s3w-object-validation-error-envelopes#task#8 (PutObject, 400 -> 400): $.Error.Code value_mismatch (expected 'InvalidStorageClass', got 'InvalidArgument'); Message value_mismatch (expected 'The storage class you specified is not valid'); StorageClassRequested missing
s3-bulk-object-delete-and-bucket-service-settings#task#18 (DeleteObjects, 400 -> 400): $.Error.Message value_mismatch — the emulator appends ': a Delete request carries from 1 to 1000 Object elements, and this one carries 1001'

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: CreateBucketMetadataConfiguration with RecordExpiration ENABLED and Days below 7; PutObjectAcl with both x-amz-acl and x-amz-grant-read; CopyObject with source == destination and no metadata change, and with an unsupported x-amz-metadata-directive; ListObjectsV2 with a corrupt continuation-token and with encoding-type=bogus; ListObjectAnnotations with max-annotation-results=0; PutObject with a 1025-byte key and with x-amz-storage-class: BOGUS; DeleteObjects with 1001 Object elements. Compare Code, Message and the argument-naming members of each refusal against AWS.
