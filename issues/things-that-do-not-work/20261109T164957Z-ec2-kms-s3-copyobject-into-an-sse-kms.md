Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__kms__s3.directory-bucket-sse-kms-customer-managed-key-round-trips
Issue-Key: ec2__kms__s3.directory-bucket-sse-kms-customer-managed-key-round-trips|run#24|AssertionError|assert copy.get("BucketKeyEnabled") is False, (
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# ec2__kms__s3: CopyObject into an SSE-KMS directory bucket reports BucketKeyEnabled true, where AWS reports it false in the CopyObject response

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyObject

## Description
Cause 1 of 1: the CopyObject response projects the stored object's bucket-key setting, where AWS reports BucketKeyEnabled false in that response even though the copied object is stored with the bucket key enabled.
AWS: recorded behavior only — the S3 model declares x-amz-server-side-encryption-bucket-key-enabled on CopyObject's output but not its value. For a directory bucket whose default encryption is SSE-KMS with a customer managed key and BucketKeyEnabled true, the recording shows CopyObject answering with ServerSideEncryption 'aws:kms' and SSEKMSKeyId = the key ARN but BucketKeyEnabled false, while a subsequent HeadObject on the copy reports the object with the bucket key enabled. CreateSession, PutObject and HeadObject all report 'aws:kms' with the same key ARN.
Emulator: the same CopyObject answers BucketKeyEnabled true, so the response does not match what AWS returns for the identical request.
Why: a client that reads the copy response's bucket-key flag (to decide whether a per-object KMS call happened, or to mirror the response into its own metadata) records a value AWS never returns for a copy, and a conformance test over the copy response fails.
Fix: the CopyObject response serializer for SSE-KMS objects (the bucket-key member of the copy result, not the stored object's state).
Traces: ec2__kms__s3.directory-bucket-sse-kms-customer-managed-key-round-trips.

## Evidence
Expected: CopyObject response BucketKeyEnabled is False (the check notes the copied object is nonetheless stored with it True, confirmed by the following HeadObject). Actual AssertionError: "CopyObject reported BucketKeyEnabled True, expected False". The preceding assertions on the same response (ServerSideEncryption 'aws:kms', SSEKMSKeyId = key ARN) passed.

## Steps to Reproduce
1. Create a directory bucket (S3 Express One Zone) and a symmetric customer managed KMS key.
2. PutBucketEncryption on the bucket with Rules[0] = {ApplyServerSideEncryptionByDefault: {SSEAlgorithm: 'aws:kms', KMSMasterKeyID: <key ARN>}, BucketKeyEnabled: True}.
3. PutObject an object, then CopyObject it to a second key in the same bucket and read the CopyObject response: ServerSideEncryption 'aws:kms', SSEKMSKeyId = <key ARN>, BucketKeyEnabled False. HeadObject on the copy then reports the object under the same key.
