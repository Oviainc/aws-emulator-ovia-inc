Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-direct-put-batch-delivers-records-to-s3
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-direct-put-batch-delivers-records-to-s3
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-crc32
Issue-Related: 20261109T175957Z-s3-getobject-serves-the-appflow-written-object-with.md
Issue-Commit: 684a6e0566cc639f51ebd62e86c005df2b14ef8a

# S3 GetObject omits the CRC32 full-object checksum headers for a Firehose-delivered object

> _Note: this issue was identified against the code base at commit `684a6e0566cc639f51ebd62e86c005df2b14ef8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One net-new root cause in the S3 GetObject response-header serializer: the stored object's checksum pair (the algorithm-specific checksum header and x-amz-checksum-type) is not served for an object that was written with a CRC32 full-object checksum — here the delivery object a Firehose DirectPut stream writes into its S3 destination bucket.

Cause 1 of 1: GetObject returns neither the x-amz-checksum-crc32 header nor x-amz-checksum-type: FULL_OBJECT.
AWS: Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): GetObject carries @httpChecksum with requestValidationModeMember ChecksumMode and responseAlgorithms [CRC64NVME, CRC32, CRC32C, SHA256, SHA1, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128]; GetObjectOutput member ChecksumCRC32 is bound to the x-amz-checksum-crc32 header and member ChecksumType (enum com.amazonaws.s3#ChecksumType, values COMPOSITE | FULL_OBJECT) to the x-amz-checksum-type header. When checksum validation is enabled on the request, S3 returns the header for the algorithm the object was actually stored with, plus the checksum type describing whether that value covers the whole object (FULL_OBJECT) or is a composite of part checksums (COMPOSITE). A single-request (non-multipart) upload carrying a CRC32 checksum is stored FULL_OBJECT, so both headers come back. The checksum header value is the object's own base64 CRC32 computed over its bytes and is per-object (the recording does not assert a literal here, only its presence); 'FULL_OBJECT' is fixed text from the enum.
Emulator: the 200 response carries neither header — x-amz-checksum-crc32 is absent (expected: the object's base64 CRC32) and x-amz-checksum-type is absent (expected the fixed string 'FULL_OBJECT'). The body and status are correct.
Why: a client that verifies integrity on download reads the checksum header for the algorithm the object was stored with and uses x-amz-checksum-type to decide whether that value may be compared against a checksum of the full byte range or must be recomputed per part. With both headers absent the download cannot be verified at all, and the client cannot distinguish an object stored without a checksum from a dropped header — so SDK response-checksum validation silently degrades to no validation for every Firehose-delivered object.
Fix: the S3 GetObject/HeadObject checksum response-header serializer (it must emit the stored algorithm's x-amz-checksum-* member and the ChecksumType member), together with the S3 object-write path/state that must persist the algorithm, the computed value and the FULL_OBJECT checksum type when an upload supplies a CRC32 checksum.
Traces: firehose-direct-put-batch-delivers-records-to-s3#task#8.

Related-but-not-covering open issues: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md, 20261109T165919Z-s3-getobject-drops-the-transfer-written-object-s.md and 20261109T175957Z-s3-getobject-serves-the-appflow-written-object-with.md all carry only the x-amz-checksum-crc64nvme GetObject header key (the default-algorithm case) and none carries the x-amz-checksum-crc32 or x-amz-checksum-type keys or this fixture step, so none covers this cause. 20261030T053639Z-s3-listobjects-and-listobjectsv2-omit-prefix-marker-and.md is listing-body side only.

## Evidence
Replay verdict 'mismatch', expected_status 200, actual_status 200 (status and body match; the response headers diverge).
Fixture firehose-direct-put-batch-delivers-records-to-s3, step task#8, operation GetObject, service s3, cluster size 1 (affected: firehose-direct-put-batch-delivers-records-to-s3#task#8), stub_path empty, owned_by empty.
detail:
  $headers.x-amz-checksum-crc32: missing (expected '<shaped-not-asserted>', got None)
  $headers.x-amz-checksum-type: missing (expected 'FULL_OBJECT', got None)
divergence_key: service=s3|GetObject|status=200->200|keys=$headers.x-amz-checksum-crc32,$headers.x-amz-checksum-type
Model evidence: smithy_query query=operation operation=GetObject and query=shape shape=ChecksumType on services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json — GetObjectOutput.ChecksumCRC32 -> httpHeader 'x-amz-checksum-crc32', GetObjectOutput.ChecksumType -> httpHeader 'x-amz-checksum-type' with enum values COMPOSITE | FULL_OBJECT, httpChecksum.responseAlgorithms includes CRC32, input member ChecksumMode -> httpHeader 'x-amz-checksum-mode' (enum ENABLED).
Corroborating signal in the same fixture: the ListObjectsV2 step over the same bucket also reports ChecksumAlgorithm 'CRC32' and ChecksumType 'FULL_OBJECT' missing, i.e. the stored checksum metadata is absent from state on both the listing and the object-read surface.

## Steps to Reproduce
1. Replay the wire corpus and re-run the binding in Issue-Trace/Issue-Step: firehose-direct-put-batch-delivers-records-to-s3#task#8.
2. By hand: create an S3 bucket and a Firehose DirectPut delivery stream with that bucket as its S3 destination, PutRecordBatch some records and wait for the delivery object to appear under the stream's key prefix (equivalently: PUT an object in a single request supplying a CRC32 checksum via x-amz-sdk-checksum-algorithm: CRC32 / x-amz-checksum-crc32).
3. GET that object with x-amz-checksum-mode: ENABLED and inspect the 200 response headers.
4. Expected: x-amz-checksum-crc32 carries the object's base64 CRC32 over its bytes and x-amz-checksum-type carries 'FULL_OBJECT'. Observed: both headers are absent.
