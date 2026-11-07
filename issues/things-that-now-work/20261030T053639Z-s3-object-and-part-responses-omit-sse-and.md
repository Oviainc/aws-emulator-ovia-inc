Issue-Source: trace-sweep-divergence
Issue-Trace: s3-multipart-upload-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-multipart-upload-lifecycle s3-object-copy-and-rename s3w-multipart-complete-errors s3w-multipart-entity-too-small s3w-object-annotation-validation-errors
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CreateMultipartUpload|2xx->2xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|UploadPart|2xx->2xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|UploadPart|2xx->2xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|CompleteMultipartUpload|2xx->2xx|$.CompleteMultipartUploadResult.ChecksumCRC64NVME
Issue-Key: s3|CompleteMultipartUpload|2xx->2xx|$.CompleteMultipartUploadResult.ChecksumType
Issue-Key: s3|CompleteMultipartUpload|2xx->2xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|HeadObject|2xx->2xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|UploadPart|2xx->2xx|$.ETag
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-crc64nvme
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-type
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|PutObjectAnnotation|2xx->2xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|PutObjectAnnotation|2xx->2xx|$headers.x-amz-checksum-type
Issue-Key: s3|GetObjectAnnotation|2xx->2xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|GetObjectAnnotation|2xx->2xx|$headers.x-amz-checksum-type
Issue-Key: s3|ListObjectAnnotations|2xx->2xx|$.ListObjectAnnotationsOutput.Annotations.AnnotationEntry.ChecksumAlgorithm
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 object and part responses omit SSE and checksum headers

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMultipartUpload

## Description
Ten net-new root causes sharing one fault: the emulator never reports the server-side-encryption state or the computed checksum of what it stored, so x-amz-server-side-encryption, the x-amz-checksum-* headers, x-amz-checksum-type and the body checksum members are all absent from otherwise successful responses. A fixer must close all ten.

AWS (common): every object/part write and read reports the encryption actually applied in x-amz-server-side-encryption (AES256 for SSE-S3, which is the default on a bucket with default encryption), and reports the checksum it computed over the stored bytes in the matching x-amz-checksum-<alg> header plus x-amz-checksum-type. Smithy: CreateMultipartUploadOutput, UploadPartOutput, PutObjectOutput, GetObjectOutput, HeadObjectOutput and CompleteMultipartUploadOutput all model ServerSideEncryption -> x-amz-server-side-encryption and ChecksumCRC32/CRC32C/CRC64NVME/SHA1/SHA256/… -> x-amz-checksum-*, with ChecksumType -> x-amz-checksum-type; CompleteMultipartUploadOutput carries ChecksumCRC64NVME and ChecksumType in the XML body. UploadPart declares httpChecksum{requestAlgorithmMember: ChecksumAlgorithm}.

Why (common): the checksum header is the client's only way to verify end-to-end integrity of what the service stored, and the SSE header is how a client confirms (and audits) that the object was encrypted at rest. A client that compares its locally computed digest against the response header cannot, and one that asserts encryption-at-rest sees an unencrypted-looking object. The open behavioral issue 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md covers CopyObject/UploadPart checksum computation but carries none of these wire replay keys and does not cover the SSE header at all.

Cause 1 of 10: CreateMultipartUpload omits x-amz-server-side-encryption.
Fix: the CreateMultipartUpload serializer, which must report the bucket's resolved default encryption.
Traces: s3-multipart-upload-lifecycle#task#3 (also s3-multipart-upload-lifecycle#task#10, s3-object-copy-and-rename#task#5, s3w-multipart-complete-errors#task#3, s3w-multipart-entity-too-small#task#2).

Cause 2 of 10: UploadPart omits x-amz-server-side-encryption.
Fix: the UploadPart serializer.
Traces: s3-multipart-upload-lifecycle#task#4 (also #task#11).

Cause 3 of 10: UploadPart omits both x-amz-checksum-crc32 and x-amz-server-side-encryption when a CRC32 was requested.
Fix: the UploadPart handler's checksum computation over the part bytes and its header serialization.
Traces: s3w-multipart-complete-errors#task#4 (also #task#5).

Cause 4 of 10: CompleteMultipartUpload omits ChecksumCRC64NVME and ChecksumType from the result body and x-amz-server-side-encryption from the headers.
Fix: the CompleteMultipartUpload handler's full-object checksum computation and its serializer.
Traces: s3w-multipart-complete-errors#task#8 (also s3w-multipart-entity-too-small#task#6).

Cause 5 of 10: HeadObject omits x-amz-server-side-encryption for an object stored under default encryption.
Fix: the HeadObject metadata serializer.
Traces: s3w-multipart-complete-errors#task#9.

Cause 6 of 10: UploadPart omits the ETag output member together with x-amz-checksum-crc32 and x-amz-server-side-encryption.
AWS: UploadPartOutput.ETag is bound to the ETag response header and is what the client must echo back in CompleteMultipartUpload.
Emulator: all three absent.
Fix: the UploadPart serializer (ETag, checksum and SSE headers).
Traces: s3w-multipart-entity-too-small#task#3 (also #task#4).

Cause 7 of 10: GetObject omits x-amz-checksum-crc64nvme, x-amz-checksum-type and x-amz-server-side-encryption for a multipart-assembled object.
Fix: the GetObject metadata serializer and the stored full-object checksum it must read.
Traces: s3w-multipart-entity-too-small#task#7.

Cause 8 of 10: PutObjectAnnotation omits x-amz-checksum-crc32 and x-amz-checksum-type.
Fix: the PutObjectAnnotation handler's checksum computation and header serialization.
Traces: s3w-object-annotation-validation-errors#task#3.

Cause 9 of 10: GetObjectAnnotation omits x-amz-checksum-crc32 and x-amz-checksum-type.
Fix: the GetObjectAnnotation serializer and the stored annotation checksum.
Traces: s3w-object-annotation-validation-errors#task#4.

Cause 10 of 10: ListObjectAnnotations omits ChecksumAlgorithm on each AnnotationEntry.
Fix: the ListObjectAnnotations serializer and the per-annotation checksum metadata.
Traces: s3w-object-annotation-validation-errors#task#9

## Evidence
All ten clusters are 'mismatch' with matching status 200.
s3-multipart-upload-lifecycle#task#3 (CreateMultipartUpload): $headers.x-amz-server-side-encryption missing (expected 'AES256')
s3-multipart-upload-lifecycle#task#4 (UploadPart): $headers.x-amz-server-side-encryption missing (expected 'AES256')
s3w-multipart-complete-errors#task#4 (UploadPart): $headers.x-amz-checksum-crc32 missing; $headers.x-amz-server-side-encryption missing
s3w-multipart-complete-errors#task#8 (CompleteMultipartUpload): $.CompleteMultipartUploadResult.ChecksumCRC64NVME missing; .ChecksumType missing (expected 'FULL_OBJECT'); $headers.x-amz-server-side-encryption missing
s3w-multipart-complete-errors#task#9 (HeadObject): $headers.x-amz-server-side-encryption missing (expected 'AES256')
s3w-multipart-entity-too-small#task#3 (UploadPart): $.ETag missing; $headers.x-amz-checksum-crc32 missing; $headers.x-amz-server-side-encryption missing
s3w-multipart-entity-too-small#task#7 (GetObject): $headers.x-amz-checksum-crc64nvme missing; $headers.x-amz-checksum-type missing (expected 'FULL_OBJECT'); $headers.x-amz-server-side-encryption missing
s3w-object-annotation-validation-errors#task#3 (PutObjectAnnotation): $headers.x-amz-checksum-crc32 missing; $headers.x-amz-checksum-type missing (expected 'FULL_OBJECT')
s3w-object-annotation-validation-errors#task#4 (GetObjectAnnotation): same two headers missing
s3w-object-annotation-validation-errors#task#9 (ListObjectAnnotations): $.ListObjectAnnotationsOutput.Annotations.AnnotationEntry.ChecksumAlgorithm missing (expected 'CRC32')

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand, on a bucket with SSE-S3 default encryption: CreateMultipartUpload, UploadPart (with and without x-amz-sdk-checksum-algorithm CRC32) and CompleteMultipartUpload, then HeadObject and GetObject the assembled key; PutObjectAnnotation and GetObjectAnnotation with a CRC32 checksum, then ListObjectAnnotations. Check for x-amz-server-side-encryption: AES256, the matching x-amz-checksum-* header, x-amz-checksum-type, UploadPart's ETag header, and CompleteMultipartUpload's ChecksumCRC64NVME/ChecksumType body members.
