Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3__sqs.xsvc-s3-2-notification-config-to-a-kms-encrypted-queue-is-rejected-until-the-key-policy-grants-this-bucket
Issue-Key: kms__s3__sqs.xsvc-s3-2-notification-config-to-a-kms-encrypted-queue-is-rejected-until-the-key-policy-grants-this-bucket|_config_error#24|AssertionError|raise AssertionError(
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms__s3__sqs: PutBucketNotificationConfiguration does not validate an SSE-KMS queue destination — a configuration whose CMK lets S3 encrypt nothing is accepted instead of refused 400 InvalidArgument (1 cause)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
ONE cause.

Cause 1 of 1 — S3 accepts a bucket notification configuration naming an SSE-KMS queue it cannot encrypt for, instead of validating the destination against KMS and refusing the whole call.
AWS (recorded behavior; S3's destination validation is not expressed in the model): PutBucketNotificationConfiguration validates every destination synchronously by sending an s3:TestEvent. For a queue encrypted with a customer CMK whose key policy does not let s3.amazonaws.com encrypt for it — including when the grant exists but is scoped to a DIFFERENT bucket's aws:SourceArn — the WHOLE call fails with HTTP 400, Error.Code 'InvalidArgument', Error.Message 'Unable to validate the following destination configurations' (fixed text), ArgumentName1 = the offending destination's queue ARN (from the request) and ArgumentValue1 = 'The message to the SSE queue could not be encrypted using KMS' (fixed text). The rejection is atomic: the bucket keeps exactly its prior configuration. Granting s3.amazonaws.com kms:GenerateDataKey+kms:Decrypt with an aws:SourceArn condition naming THIS bucket makes the identical call succeed at once, and the TestEvent then arrives on the queue.
Emulator: the call returns success and the configuration is stored (the check's _config_error reports 'PutBucketNotificationConfiguration ACCEPTED [control, kms-subject] although S3 cannot encrypt for the SSE-KMS queue under the current key policy'). The emulator performs no TestEvent, no SQS/KMS encryption attempt and no cross-service verdict, so the key policy — and the aws:SourceArn condition in it — is never evaluated. The unencrypted control queue's configuration was accepted correctly first, and the queue's KmsMasterKeyId read back as the CMK, so only the validation is missing.
Why: this refusal is the only point at which a customer learns that a notification wiring will never deliver. Accepting it makes a misconfigured key policy look healthy and converts a loud 400 at configure time into silent, permanent event loss at runtime; the atomicity contract (a rejected call must not partially apply) is also unverifiable.
Fix: the S3 PutBucketNotificationConfiguration handler's destination validation — attempt the test delivery for each destination (SQS asking KMS for a data key as the s3.amazonaws.com principal, with the bucket's aws:SourceArn), fail the whole request 400 InvalidArgument with the ArgumentName1/ArgumentValue1 pair when any destination cannot be encrypted for, and leave the stored configuration untouched.
Traces: kms__s3__sqs.xsvc-s3-2-notification-config-to-a-kms-encrypted-queue-is-rejected-until-the-key-policy-grants-this-bucket.

## Evidence
"AssertionError: PutBucketNotificationConfiguration ACCEPTED ['control', 'kms-subject'] although S3 cannot encrypt for the SSE-KMS queue under the current key policy" (raised by the check's _config_error helper, which only raises when the call did NOT fail). The check's assertions name the required refusal exactly: Error.Code 'InvalidArgument', Error.Message 'Unable to validate the following destination configurations', ArgumentName1 == the SSE-KMS queue's ARN (and not the control queue's), ArgumentValue1 == 'The message to the SSE queue could not be encrypted using KMS'; then that the bucket's configuration still reads back as exactly ['control']. The check's own measurement notes that real AWS returns this identically for 60 s of retries, so it is not policy propagation, and that granting s3.amazonaws.com GenerateDataKey+Decrypt scoped to the bucket makes the identical call succeed at once. Earlier legs passed: the control (unencrypted) queue configuration was accepted and read back as ['control'], and the SSE-KMS queue's KmsMasterKeyId attribute echoed the CMK ARN.

## Steps to Reproduce
CreateKey; create two SQS queues, one with KmsMasterKeyId=<key ARN> and one with SqsManagedSseEnabled=false; give both an SQS resource policy allowing s3.amazonaws.com SendMessage from the bucket; create the bucket; PutBucketNotificationConfiguration with only the unencrypted queue (succeeds); then PutBucketNotificationConfiguration with BOTH queue configurations while the CMK's key policy grants s3.amazonaws.com nothing — observe success instead of HTTP 400 InvalidArgument / 'Unable to validate the following destination configurations' naming the SSE-KMS queue ARN with reason 'The message to the SSE queue could not be encrypted using KMS'. Repeat with the KMS grant present but conditioned on a different bucket's aws:SourceArn — also wrongly accepted.
