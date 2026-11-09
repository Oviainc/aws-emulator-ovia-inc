Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3__sts.s3-put-bucket-encryption-requires-putencryptionconfiguration-action
Issue-Key: iam__s3__sts.s3-put-bucket-encryption-requires-putencryptionconfiguration-action|_error#16|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# S3 PutBucketEncryption is not authorized against s3:PutEncryptionConfiguration: a principal without that action still changes the bucket's default encryption

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketEncryption

## Description
Cause 1 of 1: PutBucketEncryption is served without charging the s3:PutEncryptionConfiguration action against the caller's policy, so a principal explicitly denied (or simply not granted) that action can still rewrite the bucket's default encryption.

AWS: PutBucketEncryption requires s3:PutEncryptionConfiguration on the bucket. A caller whose policy does not allow it — or whose policy denies it — is refused with HTTP 403 `AccessDenied`; S3's same-account AccessDenied message names why the request was refused (`User: <principal arn> is not authorized to perform: s3:PutEncryptionConfiguration on resource: "<bucket arn>" because no identity-based policy allows the s3:PutEncryptionConfiguration action`, or the explicit-deny variant). The fixed text is that standard wording; principal ARN, action and bucket ARN come from the request. Smithy: s3#PutBucketEncryption is an authorized bucket-level operation served over the REST-XML wire; the action mapping is a service fact — recorded behavior only, and the recording shows the un-granted call refused with 403.

Emulator: the call returns HTTP 200 with an empty body and the bucket's default encryption is changed.

Why: default-encryption posture is a compliance control. A client that restricts who may change it has no control: any caller that can reach the bucket can switch SSE-KMS back to SSE-S3 (or to a key it chooses), and the permission boundary the client tested is inert. The allowed leg also succeeds, so an enforced policy is indistinguishable from an unenforced one.

Fix: the authorization step of the PutBucketEncryption handler must charge s3:PutEncryptionConfiguration (and the matching GetEncryptionConfiguration for reads) before mutating bucket state.

Traces: iam__s3__sts.s3-put-bucket-encryption-requires-putencryptionconfiguration-action.

## Evidence
Expected: PutBucketEncryption refused with 403 AccessDenied for a principal without s3:PutEncryptionConfiguration.
Actual: AssertionError: expected the call to be rejected, but it succeeded: {… 'HTTPStatusCode': 200 …}

## Steps to Reproduce
1. Create a bucket (default SSE-S3) and a role whose policy omits/denies s3:PutEncryptionConfiguration; AssumeRole.
2. Call PutBucketEncryption to switch the default to SSE-KMS (or back to AES256).
3. Observe HTTP 200 and the changed configuration.
