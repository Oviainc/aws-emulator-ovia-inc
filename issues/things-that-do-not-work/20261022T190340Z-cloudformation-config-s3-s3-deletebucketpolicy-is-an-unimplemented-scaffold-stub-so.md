Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed
Issue-Key: cloudformation__config__s3|DeleteBucketPolicy|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# S3 DeleteBucketPolicy is an unimplemented scaffold stub, so the Config delivery-denied-after-policy-removal flow cannot be exercised

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBucketPolicy

## Description
ONE CAUSE.

Cause 1 of 1: S3 DeleteBucketPolicy answers the emulator-specific error code `NotImplemented`.
AWS: DeleteBucketPolicy removes the bucket's policy and answers 204 No Content for a bucket the caller owns; a subsequent GetBucketPolicy then fails with NoSuchBucketPolicy. It is the only way to withdraw the Config delivery grant from a delivery bucket, which is the precondition this check needs in order to observe that Config delivery is then denied.
Emulator: the call is rejected with HTTP error carrying Error.Code "NotImplemented" and message "operation DeleteBucketPolicy is scaffolded but not implemented in this emulator", so the check aborts before reaching Config at all.
Why: a client cannot remove a bucket policy, so no flow that revokes a grant and asserts the consequence can run; the returned error code does not exist in the S3 API, so SDK error handling cannot classify it. It also means the positive and negative halves of Config's delivery-permission behaviour are indistinguishable.
Fix: the S3 DeleteBucketPolicy handler and the bucket-policy state it mutates (services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket_policy.rs). Note: the suite's auto-filed scaffold-stub issues for this run cover other operations (e.g. s3 PutBucketNotificationConfiguration is filed as 20261022T154846Z-s3-putbucketnotificationconfiguration-stub); no open issue names DeleteBucketPolicy.
Traces: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false).

cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteBucketPolicy operation: operation DeleteBucketPolicy is scaffolded but not implemented in this emulator

The check reaches DeleteBucketPolicy after creating the delivery bucket and applying the three AWSConfigBucket* policy statements; the stub aborts it, so nothing about Config's delivery-denial behaviour is exercised.

This check's service token is the composite suite token `cloudformation__config__s3`; the operation at fault belongs to the s3 service crate (service-com-amazonaws-s3-amazons3), which is bootstrapped.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-replay cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed.
3. Or directly: s3 CreateBucket, s3 PutBucketPolicy with the Config delivery statements, then s3 DeleteBucketPolicy — the delete returns Error.Code NotImplemented instead of succeeding (204) and leaving GetBucketPolicy with NoSuchBucketPolicy.
