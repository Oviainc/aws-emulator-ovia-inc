Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied
Issue-Key: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied|run#15|AssertionError|assert (put_kms.get("ServerSideEncryption"), put_kms.get("SSEKMSKeyId")) == ("aws:kms", key_arn), (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# S3 PutObject ignores the bucket's default SSE-KMS encryption configuration: the response reports no encryption at all

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Cause 1 of 1: an object written to a bucket whose default encryption configuration is SSE-KMS with a customer managed key is not encrypted with it, and the PutObject response omits the encryption members entirely.
AWS: Smithy: PutObject's output members ServerSideEncryption (httpHeader x-amz-server-side-encryption) and SSEKMSKeyId (httpHeader x-amz-server-side-encryption-aws-kms-key-id) are echoed for an object that is SSE-KMS encrypted; when PutBucketEncryption has set a rule with ApplyServerSideEncryptionByDefault={'SSEAlgorithm':'aws:kms','KMSMasterKeyID':<cmk arn>}, a PutObject that specifies no encryption headers inherits that default and the response reports ('aws:kms', <cmk arn>).
Emulator: PutBucketEncryption is accepted, but the subsequent PutObject response carries neither header — (ServerSideEncryption, SSEKMSKeyId) is (None, None) — so the bucket default is not applied and no KMS key is bound to the object.
Why: a client cannot rely on encryption-at-rest defaults: an object a compliance control believes is CMK-encrypted is not, and everything downstream that depends on the key binding (KMS key-policy enforcement on read, grants, 'bucket keys off' behavior per request) is unverifiable.
Fix: the PutObject handler in the s3 crate — resolve the bucket's ServerSideEncryptionConfiguration when the request specifies no SSE members, record the algorithm/key on the object's metadata state, and echo both response headers (GetObject/HeadObject must report the same). The open issue 20261022T210606Z-s3-breaks-cloudformation-provisioned-buckets-and-refuses-two.md covers PutObject with EXPLICIT SSE headers answering NotImplemented; it does not cover inheriting the bucket default and does not carry this replay key.
Traces: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied.

## Evidence
"AssertionError: the bucket default must encrypt with the CMK, got (None, None)" — raised by assert (put_kms.get('ServerSideEncryption'), put_kms.get('SSEKMSKeyId')) == ('aws:kms', key_arn) on the first put_object after s3.put_bucket_encryption(Bucket=bucket, ServerSideEncryptionConfiguration={'Rules':[{'ApplyServerSideEncryptionByDefault':{'SSEAlgorithm':'aws:kms','KMSMasterKeyID':key_arn},'BucketKeyEnabled':False}]}).

## Steps to Reproduce
1. Create a KMS customer managed key and a bucket.
2. PutBucketEncryption(Bucket=..., ServerSideEncryptionConfiguration={'Rules':[{'ApplyServerSideEncryptionByDefault':{'SSEAlgorithm':'aws:kms','KMSMasterKeyID':<key arn>},'BucketKeyEnabled':False}]}).
3. PutObject(Bucket=..., Key='kms/asset.txt', Body=b'...') with no SSE headers -> expect the response to report ServerSideEncryption='aws:kms' and SSEKMSKeyId=<key arn>; observe both absent.
