Issue-Source: trace-sweep-divergence
Issue-Trace: s3w-object-validation-error-envelopes
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3w-object-validation-error-envelopes s3-txt-conditional-date-headers-and-precedence s3-mvb-rename-object-general-purpose s3-metadata-config-with-inventory-and-annotation
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|4xx->2xx|$.Error
Issue-Key: s3|PutObject|4xx->2xx|status
Issue-Key: s3|GetObject|2xx->3xx|$
Issue-Key: s3|GetObject|2xx->3xx|$headers.accept-ranges
Issue-Key: s3|GetObject|2xx->3xx|$headers.content-length
Issue-Key: s3|GetObject|2xx->3xx|$headers.content-type
Issue-Key: s3|GetObject|2xx->3xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|GetObject|2xx->3xx|$headers.x-amz-checksum-type
Issue-Key: s3|GetObject|2xx->3xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|GetObject|2xx->3xx|status
Issue-Key: s3|RenameObject|5xx->5xx|$.Error.Header
Issue-Key: s3|RenameObject|5xx->5xx|$.Error.Message
Issue-Key: s3|GetBucketMetadataConfiguration|2xx->2xx|$.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.AnnotationTableConfigurationResult.Error.ErrorMessage
Issue-Commit: f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11

# s3 object-level requests are mis-evaluated and their error envelopes diverge: unsupported storage class accepted, conditional-header precedence ignored, wrong NotImplemented and annotation messages

> _Note: this issue was identified against the code base at commit `f0b2a1a4859373de2ddca21e0cf7a4e6a954ae11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Four net-new object-level divergences in the s3 emulator: one request AWS refuses is accepted, one AWS serves is refused, and two refusal messages are emulator-authored rather than AWS's.

Cause 1 of 4 — PutObject accepts a storage class that is not allowed for the target bucket.
AWS: Smithy: PutObjectRequest$StorageClass is bound to httpHeader x-amz-storage-class, target enum StorageClass whose values include FSX_OPENZFS, FSX_ONTAP, SNOW, EXPRESS_ONEZONE and the AWS_BACKUP_* classes. Membership in the enum does not make a class usable on a general purpose bucket: when x-amz-storage-class names a class the bucket cannot accept, S3 refuses with HTTP 400 and the envelope Code 'InvalidArgument', Message '<storage-class> is not allowed.', ArgumentName 'x-amz-storage-class', ArgumentValue '<storage-class>' (recorded response). The fixed text is ' is not allowed.'; the leading token, and both ArgumentName/ArgumentValue, are taken from the request's x-amz-storage-class header — the recorded FSX_OPENZFS is one filler for that slot.
Emulator: answers HTTP 200 and stores the object, with no Error element at all.
Why: the emulator silently accepts a storage class the real service rejects, so an object the client believes landed in a valid class exists in the emulator and does not exist in AWS. A client whose error handling is built around InvalidArgument/ArgumentName never exercises it, and a validation test of the storage-class surface passes against the emulator and fails in production.
Fix: PutObject's request validation — validate x-amz-storage-class against the classes allowed for the target bucket type before the write, and emit the InvalidArgument envelope with ArgumentName/ArgumentValue.
Traces: s3w-object-validation-error-envelopes#task#9.

Cause 2 of 4 — GetObject answers 304 where AWS serves 200, because conditional-header precedence is not applied.
AWS: Smithy: GetObjectRequest$IfModifiedSince documentation states the rule explicitly — "If both of the If-None-Match and If-Modified-Since headers are present in the request as follows: If-None-Match condition evaluates to false, and; If-Modified-Since condition evaluates to true; then, S3 returns 304 Not Modified status code" (and refers to RFC 7232). A 304 therefore requires BOTH conditions; when the entity-tag condition does not hold, the date condition is ignored and S3 serves the object with HTTP 200, its body, and the full object response headers (accept-ranges, content-type, content-length, x-amz-server-side-encryption, and the object's checksum headers x-amz-checksum-<alg>/x-amz-checksum-type).
Emulator: returns HTTP 304 with only an ETag, evaluating the If-Modified-Since date on its own and dropping the body and every object response header.
Why: a client that sends a conditional GET with both an entity-tag and a date precondition — the normal cache-revalidation pattern, and exactly what an SDK's conditional read emits — is told the object is unmodified and gets no bytes, so it keeps serving stale content and never observes the current object, its content type, its encryption report or its checksum.
Fix: GetObject's conditional-request evaluation in the handler — apply RFC 7232 / S3 precedence so If-None-Match decides over If-Modified-Since (and If-Match over If-Unmodified-Since) before any 304 is returned.
Traces: s3-txt-conditional-date-headers-and-precedence#task#4.

Cause 3 of 4 — RenameObject's 501 refusal on a general purpose bucket uses the query-parameter message and omits the Header element.
AWS: Smithy: RenameObject is bound to PUT /{Bucket}/{Key+}?renameObject with required input member RenameSource on httpHeader x-amz-rename-source. RenameObject is not available for general purpose buckets, and S3 attributes the refusal to the HEADER that requested it: HTTP 501 with Code 'NotImplemented', Message 'A header you provided implies functionality that is not implemented', and an error element Header naming the offending header, 'x-amz-rename-source' (recorded response). The message is fixed text; the Header element value is the request header that triggered it.
Emulator: answers HTTP 501 but with Message 'A query parameter you provided implies functionality that is not implemented' and no Header element at all.
Why: both texts are real S3 NotImplemented messages for DIFFERENT causes. A client (or an operator reading the failure) is told the unsupported thing was a query parameter and is given nothing identifying it, so it looks for a bad ?renameObject rather than the x-amz-rename-source header; error handling that branches on the Error/Header element finds it absent.
Fix: the RenameObject handler's refusal path — select the header-attributed NotImplemented message and serialize the Header element with the request's x-amz-rename-source.
Traces: s3-mvb-rename-object-general-purpose#task#3.

Cause 4 of 4 — GetBucketMetadataConfiguration reports the annotation-table permission failure with an emulator-authored message.
AWS: recorded behavior only — the message is not modelled. In the GetBucketMetadataConfiguration 200 result, MetadataConfigurationResult/AnnotationTableConfigurationResult/Error/ErrorMessage carries S3's fixed diagnostic for an annotation role that cannot be used: 'The annotation role does not have sufficient permissions. Ensure the role grants s3:GetObjectAnnotation, s3:GetObjectVersionAnnotation, s3:ListBucket, and s3:ListBucketVersions on the bucket, and that the role's trust policy allows metadata.s3.amazonaws.com to assume it. If the bucket or its objects/annotations are encrypted with a KMS key, ensure the role also has kms:Decrypt permission. Additionally, ensure that the KMS key used to encrypt the bucket still exists, is active and has a resource policy granting access to the S3 service principals. To create a new S3 Metadata table, you must delete the S3 Metadata configuration for this bucket, and then create a new S3 Metadata configuration.' The whole string is fixed text for this error class — it interpolates nothing from the request or from state.
Emulator: returns a paraphrase instead: "The IAM role for the annotation table doesn't have sufficient permissions. Make sure that the role has a trust policy that allows the Amazon S3 metadata service to assume it, and a permissions policy that grants the actions needed to read annotations from your bucket. To create a new metadata table, you must delete the metadata configuration for this bucket, and then create a new metadata configuration."
Why: the AWS text is the actionable part of this result — it names the four IAM actions, the metadata.s3.amazonaws.com trust principal and the kms:Decrypt/KMS-key-state requirement. The emulator's paraphrase names none of them, so an operator (or automation matching on the message) cannot determine what to grant, and anything asserting on the documented wording diverges.
Fix: the GetBucketMetadataConfiguration result serializer / the annotation-table error strings it draws from — use S3's exact ErrorMessage text for this error class.
Traces: s3-metadata-config-with-inventory-and-annotation#task#4.

## Evidence
Replay of the wire corpus (service s3):

- PutObject, fixture s3w-object-validation-error-envelopes step task#9: expected status 400 with $.Error = {'Code': 'InvalidArgument', 'Message': 'FSX_OPENZFS is not allowed.', 'ArgumentName': 'x-amz-storage-class', 'ArgumentValue': 'FSX_OPENZFS', ...}; actual status 200 with $.Error missing.
- GetObject, fixture s3-txt-conditional-date-headers-and-precedence step task#4: expected status 200 with the object body and headers accept-ranges='bytes', content-type='text/plain', content-length, x-amz-server-side-encryption='AES256', x-amz-checksum-crc32, x-amz-checksum-type='FULL_OBJECT'; actual status 304 with a body carrying only {'ETag': ...} and all of those headers missing.
- RenameObject, fixture s3-mvb-rename-object-general-purpose step task#3, status 501 -> 501: $.Error.Message value_mismatch (expected 'A header you provided implies functionality that is not implemented', got 'A query parameter you provided implies functionality that is not implemented'); $.Error.Header missing (expected 'x-amz-rename-source', got None).
- GetBucketMetadataConfiguration, fixture s3-metadata-config-with-inventory-and-annotation step task#4, status 200 -> 200: $.GetBucketMetadataConfigurationResult.MetadataConfigurationResult.AnnotationTableConfigurationResult.Error.ErrorMessage value_mismatch — AWS's full permissions text (naming s3:GetObjectAnnotation, s3:GetObjectVersionAnnotation, s3:ListBucket, s3:ListBucketVersions, the metadata.s3.amazonaws.com trust principal and kms:Decrypt) replaced by a shorter emulator paraphrase.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. s3w-object-validation-error-envelopes#task#9: PUT an object with x-amz-storage-class: FSX_OPENZFS to a general purpose bucket -> emulator 200, AWS 400 InvalidArgument '<class> is not allowed.' with ArgumentName/ArgumentValue.
3. s3-txt-conditional-date-headers-and-precedence#task#4: GET an object with both an If-None-Match whose entity-tag condition does not hold and an If-Modified-Since that on its own would yield not-modified -> emulator 304 (ETag only), AWS 200 with the body and the object response headers.
4. s3-mvb-rename-object-general-purpose#task#3: PUT /<bucket>/<key>?renameObject with x-amz-rename-source on a general purpose bucket -> emulator 501 with the query-parameter message and no Error/Header, AWS 501 with the header message and Header: x-amz-rename-source.
5. s3-metadata-config-with-inventory-and-annotation#task#4: GET the bucket metadata configuration after an annotation table whose role lacks permissions -> compare AnnotationTableConfigurationResult/Error/ErrorMessage with AWS's text.
