Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key
Issue-Key: kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key|run#16|AssertionError|assert key_arn.startswith("arn:") and ":key/" in key_arn, (
Issue-Commit: ec4677bae9b70a2d31b2abdbe7af61c1994727a3

# S3 reports no SSE-KMS key ARN on an object written under an aws:kms bucket default that omits KMSMasterKeyID, so the AWS-managed aws/s3 key is never resolved

> _Note: this issue was identified against the code base at commit `ec4677bae9b70a2d31b2abdbe7af61c1994727a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
One net-new root cause in the kms__s3 surface, grounded in a failing behavioral check whose id leads with `kms__s3`. The check is NOT marked ALREADY FILED and NOT marked OWNED in the replay report.

----------------------------------------------------------------
Cause 1 of 1: a PutObject into a bucket whose default encryption rule is `aws:kms` with NO KMSMasterKeyID returns an empty SSE-KMS key id instead of the concrete ARN of the account's AWS-managed aws/s3 key.
Failing check: kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key

AWS: Smithy (com.amazonaws.s3) models the bucket default as ServerSideEncryptionRule{ApplyServerSideEncryptionByDefault, BucketKeyEnabled, BlockedEncryptionTypes}; ServerSideEncryptionByDefault.KMSMasterKeyID is optional, and GetBucketEncryption has no modeled error for the "no key id" case — omitting it is a legal, meaningful configuration. PutObject's output carries `ServerSideEncryption` (x-amz-server-side-encryption) and `SSEKMSKeyId` (x-amz-server-side-encryption-aws-kms-key-id). When the rule names no key, S3 encrypts under the account's AWS-managed key behind alias/aws/s3 and REPORTS that key's concrete ARN in SSEKMSKeyId (the value then resolves in KMS to a key whose KeyManager is AWS) — that reported ARN is exactly what distinguishes "SSE-KMS with the AWS-managed key" from SSE-KMS with a customer-managed key. The fixed part of the requirement is the shape of the value (an `arn:<partition>:kms:<region>:<account>:key/<key-id>`); the region, account and key id are values taken from the request's bucket and the emulator's own KMS state, never literals.
Emulator: PutObject returns ServerSideEncryption `aws:kms` but SSEKMSKeyId is the empty string — `AssertionError: the object must name the concrete key ARN it was encrypted under, got ''`. The header is emitted with no value, so the key that encrypted the object is unknowable and nothing can be resolved in KMS.
Why: a client auditing encryption posture cannot tell whether an object sits under the AWS-managed aws/s3 key or an arbitrary customer key, and any code that feeds SSEKMSKeyId into kms:DescribeKey / a key-policy or grant check (the normal way to prove an object is under a managed vs. customer key) gets an empty identifier and either crashes or silently passes. Replication, CopyObject re-encryption and cross-account reads all key off this value.
Fix: the PutObject handler's SSE-KMS resolution in the s3 crate must, when the bucket's default rule selects aws:kms with no KMSMasterKeyID, resolve alias/aws/s3 in KMS (creating the AWS-managed key on first use, KeyManager AWS) and store/serialise that key's ARN on the object so PutObject, HeadObject and GetObject all report it in x-amz-server-side-encryption-aws-kms-key-id.
Related open issue that does NOT carry this check's replay key: 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md (it owns the sibling checks where an explicit SSEKMSKeyId / alias is not resolved or validated; this cause is the no-key-id default path, which that issue's replay keys do not cover).
Traces: kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 197/749 passed), service kms__s3 2/12:

  [mismatch] kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key
    check line 50: assert key_arn.startswith("arn:") and ":key/" in key_arn
    AssertionError: the object must name the concrete key ARN it was encrypted under, got ''

Sequence the check drives: CreateBucket -> PutBucketEncryption with ApplyServerSideEncryptionByDefault{SSEAlgorithm: "aws:kms"} and no KMSMasterKeyID -> GetBucketEncryption (rule read back; KMSMasterKeyID correctly absent, BucketKeyEnabled false) -> PutObject(Key="sample.csv").

Expected: PutObject responds 200 with ServerSideEncryption "aws:kms" and SSEKMSKeyId set to the ARN of the account's AWS-managed aws/s3 key, of the form arn:aws:kms:<bucket-region>:<account-id>:key/<key-id>, which kms:DescribeKey then reports with KeyManager "AWS" and alias alias/aws/s3.
Actual:   ServerSideEncryption "aws:kms" and SSEKMSKeyId "" (empty), so no key can be resolved in KMS.

## Steps to Reproduce
Boot the emulator from this checkout and replay kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key.

1. s3.CreateBucket(Bucket=<b>) in the test region.
2. s3.PutBucketEncryption(Bucket=<b>, ServerSideEncryptionConfiguration={Rules:[{ApplyServerSideEncryptionByDefault:{SSEAlgorithm:"aws:kms"}}]})  — no KMSMasterKeyID.
3. s3.GetBucketEncryption(Bucket=<b>) -> the stored rule must still omit KMSMasterKeyID and report BucketKeyEnabled false (this part passes today).
4. s3.PutObject(Bucket=<b>, Key="sample.csv", Body=<bytes>)
   -> expect ServerSideEncryption "aws:kms" AND a non-empty SSEKMSKeyId of the form arn:aws:kms:<region>:<account>:key/<key-id>.
   -> observe SSEKMSKeyId "".
5. kms.DescribeKey(KeyId=<the reported SSEKMSKeyId>) -> expect KeyManager "AWS" for the alias/aws/s3 key; unreachable because step 4 reports no identifier.
