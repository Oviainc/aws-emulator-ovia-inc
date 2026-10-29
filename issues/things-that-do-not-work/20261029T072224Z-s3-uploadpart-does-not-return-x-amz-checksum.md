Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-full-object-crc64nvme-over-multipart-equals-the-whole-object-checksum
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-full-object-crc64nvme-over-multipart-equals-the-whole-object-checksum|_upload#13|KeyError|assert uploaded[field] == part_checksum, (
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# s3: UploadPart does not return x-amz-checksum-crc64nvme, so a CRC64NVME multipart upload cannot be assembled (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UploadPart

## Description
ONE root cause. 1 CAUSE must be closed before this issue is submitted.

NOTE ON PRIOR ART: the open issue 20261025T160428Z-s3-multipart-analytics-3-net-new-root-causes.md owns other multipart/checksum checks but does NOT carry this check's replay key, so this failure is un-gated. Filed separately and named here.

Cause 1 of 1: UploadPart with ChecksumAlgorithm=CRC64NVME and an explicit x-amz-checksum-crc64nvme header returns a response that omits that checksum header entirely, so the client cannot read back the part's CRC64NVME (botocore raises KeyError 'ChecksumCRC64NVME') and cannot build the CompleteMultipartUpload part manifest. Failing check: s3.s3-full-object-crc64nvme-over-multipart-equals-the-whole-object-checksum.

AWS: Smithy (services/service-com-amazonaws-s3-amazons3) models UploadPartOutput with a ChecksumCRC64NVME member bound to the response header x-amz-checksum-crc64nvme (alongside ChecksumCRC32, ChecksumCRC32C, ChecksumSHA1, ChecksumSHA256, ChecksumSHA512, ChecksumMD5, ChecksumXXHASH*), and UploadPartRequest accepts the same value on the request header x-amz-checksum-crc64nvme with httpChecksum.requestAlgorithmMember = ChecksumAlgorithm. When a part is uploaded with a checksum the service verifies the bytes against the supplied value and then ECHOES the stored part checksum in the response header; the returned value equals the value the client sent for those bytes. CompleteMultipartUpload takes the same per-part checksum in each Part entry, and for ChecksumType FULL_OBJECT returns a whole-object ChecksumCRC64NVME with no "-N" part suffix (the COMPOSITE form is the checksum of the concatenated raw part digests suffixed "-<nparts>"). The checksum value itself is a function of the bytes, so the requirement is that the header is PRESENT and equals the checksum of the part's bytes — not any literal digest.

Emulator: the UploadPart response for part 1 of a CreateMultipartUpload(ChecksumAlgorithm="CRC64NVME", ChecksumType="FULL_OBJECT") upload has no ChecksumCRC64NVME member at all — `uploaded[field]` raises `KeyError: 'ChecksumCRC64NVME'` in the check's `_upload` helper. CreateMultipartUpload itself echoed ChecksumType correctly (that assertion, immediately before, passed), so the gap is specifically the x-amz-checksum-crc64nvme response header on UploadPart.

Why: CRC64NVME is the default full-object checksum algorithm for modern S3 multipart uploads. A client that uploads parts with it cannot construct the Parts manifest CompleteMultipartUpload requires (each Part carries its checksum), so the upload can never be completed; SDK high-level transfer managers that read the part checksum back from UploadPart to verify integrity fail with a missing field rather than a service error, and the whole FULL_OBJECT vs COMPOSITE distinction — the only way a client learns whether the stored checksum is over the assembled object or over the part digests — becomes unobservable.

Fix: the UploadPart handler's response serialization in the s3 crate — compute/store the part's CRC64NVME (and the other modeled algorithms) and emit it on the x-amz-checksum-crc64nvme response header, carrying the value through to the part state that CompleteMultipartUpload and ListParts read so the FULL_OBJECT whole-object checksum can be derived.

Traces: s3.s3-full-object-crc64nvme-over-multipart-equals-the-whole-object-checksum (frames run() line 212 -> `_upload` line 198).

## Evidence
Expected (AWS): upload_part(Bucket, Key, UploadId, PartNumber=1, Body=<5 MiB>, ChecksumAlgorithm="CRC64NVME", ChecksumCRC64NVME=<base64 big-endian CRC-64/NVME of the part>) returns a response containing ChecksumCRC64NVME equal to the value sent (header x-amz-checksum-crc64nvme), which the client then puts in the CompleteMultipartUpload Parts manifest; the FULL_OBJECT completion returns a ChecksumCRC64NVME with no "-N" suffix equal to the CRC64NVME of the assembled bytes, and GetObject(ChecksumMode="ENABLED") reports the same value with ChecksumType "FULL_OBJECT".

Actual (emulator):
  [error] s3.s3-full-object-crc64nvme-over-multipart-equals-the-whole-object-checksum
  File "check_s3_s3_full_object_crc64nvme_over_multipart_equals_the_whole_object_checksum.py", line 212, in run -> field64, full64 = _upload(...)
  File "...", line 198, in _upload -> assert uploaded[field] == part_checksum
  KeyError: 'ChecksumCRC64NVME'
The immediately preceding assertion (CreateMultipartUpload echoed ChecksumType == "FULL_OBJECT") passed, so the upload was created with the CRC64NVME/FULL_OBJECT pair and only the UploadPart response header is missing.

Smithy: UploadPartRequest.ChecksumCRC64NVME -> request header x-amz-checksum-crc64nvme, httpChecksum{requestAlgorithmMember: ChecksumAlgorithm}; UploadPartOutput.ChecksumCRC64NVME -> response header x-amz-checksum-crc64nvme. CompleteMultipartUploadOutput also models ChecksumCRC64NVME and ChecksumType.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. CreateBucket, then CreateMultipartUpload(Bucket, Key, ChecksumAlgorithm="CRC64NVME", ChecksumType="FULL_OBJECT").
3. UploadPart for a 5 MiB part with ChecksumAlgorithm="CRC64NVME" and ChecksumCRC64NVME set to the base64 big-endian CRC-64/NVME of those bytes.
4. Inspect the response: there is no x-amz-checksum-crc64nvme header / ChecksumCRC64NVME member (expected it echoed back), so the Parts manifest for CompleteMultipartUpload cannot be built.
5. Replay: s3.s3-full-object-crc64nvme-over-multipart-equals-the-whole-object-checksum.
