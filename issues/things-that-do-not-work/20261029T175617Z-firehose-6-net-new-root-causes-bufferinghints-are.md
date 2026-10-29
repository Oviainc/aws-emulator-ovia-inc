Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.buffering-hint-interval-delays-delivery firehose.buffering-hint-size-triggers-early-flush firehose.server-side-assumes-configured-role-for-s3-put-object firehose.server-side-role-permission-enforced-on-delivery firehose.deploy-cfn-stream-delivers-to-s3 firehose.destination-retry-duration-respected firehose.start-encryption-invalid-kms-key firehose.start-encryption-nonexistent-stream-not-found
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.buffering-hint-interval-delays-delivery|run#12|AssertionError|assert not objs, f"record delivered before buffer interval elapsed: {objs}"
Issue-Key: firehose.buffering-hint-size-triggers-early-flush|run#11|AssertionError|assert not objs, f"single below-threshold record was flushed early: {objs}"
Issue-Key: firehose.server-side-assumes-configured-role-for-s3-put-object|run#16|AssertionError|assert not any(marker_broken in line for line in end_lines), (
Issue-Key: firehose.server-side-role-permission-enforced-on-delivery|run#20|AssertionError|assert not grew, (
Issue-Key: firehose.deploy-cfn-stream-delivers-to-s3|run#11|Timeout|waiter.wait(StackName=stack_name)
Issue-Key: firehose.destination-retry-duration-respected|run#24|Timeout|time.sleep(10)
Issue-Key: firehose|StartDeliveryStreamEncryption|InvalidKMSResourceException
Issue-Key: firehose.start-encryption-nonexistent-stream-not-found|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# firehose: 6 net-new root causes — BufferingHints are ignored and every record is flushed immediately, the delivery role is never authorized for the S3 write, CFN-deployed and Splunk-destination streams never back anything up, and StartDeliveryStreamEncryption's two refusal messages are misworded

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Six net-new root causes in the single-service firehose suite. Every other firehose failure in this run is already OWNED by an open issue (20261022T064119Z, 20261024T005347Z, 20261025T083505Z, 20261026T235333Z, 20261028T075033Z, 20261028T150444Z) and is excluded here.

=== Cause 1 of 6: BufferingHints are not applied — a delivered record appears immediately instead of at the end of the buffering window ===
AWS: Smithy: BufferingHints.IntervalInSeconds @range 0..900, documented "Buffer incoming data for the specified period of time, in seconds, before delivering it to the destination. The default value is 300"; BufferingHints.SizeInMBs @range 1..128, documented "Buffer incoming data to the specified size, in MiBs, before delivering it to the destination. The default value is 5." AWS behavior: with IntervalInSeconds=60/SizeInMBs set, a single small PutRecord is NOT visible in the destination bucket shortly after the put; the object appears only once the interval elapses (or the size threshold is crossed).
Emulator: the record is written to S3 within the same second as the PutRecord. One check finds an object (Size 14) listed immediately after the put; the size-threshold check finds a single below-threshold record (Size 16) already flushed.
Why: a client that reads the destination to assert "nothing has been delivered yet" — or that relies on Firehose to coalesce many small records into one object per window — sees one object per record arriving instantly. Object counts, key layout per window and any downstream batch-size assumption are all wrong, and this is the exact inverse of the previously reported "never flushes" behavior, so BufferingHints is still simply not consulted. Related open issue 20261025T083505Z-firehose-s3-delivery-ignores-bufferinghints-the-destination-role.md describes the same family but does not carry these two checks' replay keys.
Fix: the Firehose S3 delivery worker's flush scheduler — it must hold records in the per-stream buffer until the stream's effective IntervalInSeconds elapses or SizeInMBs is reached, rather than writing on every put.
Traces: firehose.buffering-hint-interval-delays-delivery, firehose.buffering-hint-size-triggers-early-flush.

=== Cause 2 of 6: the delivery stream's RoleARN is never authorized for the destination S3 write, so revoking the role's S3 permission does not stop delivery ===
AWS: recorded behavior only (the authorization is a service-side AssumeRole + S3 PutObject, not modelled). AWS behavior: Firehose performs the destination PutObject as the stream's RoleARN. Once that role's s3:PutObject is revoked, subsequent records are not written to the bucket (the failure surfaces as an S3.AccessDenied delivery error instead) and the object count under the prefix stops growing.
Emulator: records put after the role's S3 write permission is revoked are still written to the destination bucket. One check sees the revoked-role marker present in the delivered lines; the other sees the object count grow from 1 to 2 after the revoke.
Why: a client cannot use IAM to contain a delivery stream — a revoked or mis-scoped delivery role still writes, so a least-privilege regression is invisible and a security assertion in a test suite passes against the emulator and fails on AWS.
Fix: the Firehose S3 delivery worker's write path — it must evaluate the stream's RoleARN (trust + identity policy) against the target bucket/key on every flush, at flush time rather than only at CreateDeliveryStream.
Traces: firehose.server-side-assumes-configured-role-for-s3-put-object, firehose.server-side-role-permission-enforced-on-delivery.

=== Cause 3 of 6: a CloudFormation-deployed delivery stream never delivers to its S3 destination ===
AWS: recorded behavior only. A stack declaring AWS::KinesisFirehose::DeliveryStream with an S3 destination converges, and a PutRecord against the deployed stream becomes an object in the stack's bucket.
Emulator: the check polls the destination for the deployed stream's object and is killed at the 60 s cap (`Check timed out after 60.0s`) with nothing delivered. Note the resolved issue 20261025T025203Z-firehose-reopens-a-cfn-deployed-firehose-delivery-stream-never.md fixed the stream reaching ACTIVE after CREATE_COMPLETE; the delivery itself still never happens, and this failure is a timeout, not a refusal.
Why: an infrastructure-as-code pipeline validated against the emulator reports a healthy stack whose data plane produces nothing.
Fix: the CloudFormation AWS::KinesisFirehose::DeliveryStream resource provider must register the stream with the same delivery worker the API path uses (see Cause 1).
Traces: firehose.deploy-cfn-stream-delivers-to-s3.

=== Cause 4 of 6: a Splunk destination is never attempted and the failed record is never backed up to S3 with a Splunk.* error code ===
AWS: recorded behavior only (SplunkDestinationConfiguration.RetryOptions.DurationInSeconds and HECAcknowledgmentTimeoutInSeconds are modelled; the retry/backup behavior is not). AWS behavior: with an unreachable HEC endpoint, RetryOptions.DurationInSeconds=0 and S3BackupMode=FailedEventsOnly, Firehose stops retrying after the acknowledgment window and writes the failed record to the backup bucket as a JSON record whose original bytes are base64 under `rawData` and whose `errorCode` starts with `Splunk.`.
Emulator: no backup object containing the record's marker is ever written, so the check exhausts its budget (`Check timed out after 60.0s`). The retry window is therefore unobservable in either direction.
Why: a client relying on FailedEventsOnly backup to recover records a destination rejected loses them silently, and a configured retry duration has no effect at all.
Fix: the Firehose Splunk destination delivery worker and its S3 backup path (RetryOptions/HECAcknowledgmentTimeoutInSeconds honoured, failed record serialized with rawData + errorCode into the backup bucket).
Traces: firehose.destination-retry-duration-respected.

=== Cause 5 of 6: StartDeliveryStreamEncryption's InvalidKMSResourceException message is the KMS not-found sentence instead of the Firehose KMS access-denied sentence ===
AWS: Smithy: StartDeliveryStreamEncryption errors include InvalidKMSResourceException (client, HTTP 400), documented "Firehose throws this exception when an attempt to put records or to start or stop Firehose stream encryption fails. This happens when the KMS service throws one of the following exception types: AccessDeniedException, InvalidStateException, DisabledException, or NotFoundException." For a CUSTOMER_MANAGED_CMK ARN the caller cannot use, AWS answers InvalidKMSResourceException with the fixed sentence `AccessDenied by KMS while using the CMK. Check the key policy to ensure that the KMS grant related apis are allowed. Try again after the key policy has the right permissions.` — that text is entirely fixed, with no values interpolated from the request.
Emulator: the message is instead `Key '<kms-key-arn>' does not exist`, i.e. KMS's own not-found wording with the requested key ARN interpolated. The request's key ARN is a per-request value and must not appear in this message at all.
Why: a client matching on the documented CMK-policy guidance (or stripping it for display) gets KMS's internal wording, so it cannot tell a key-policy problem from a nonexistent key and its error handling branches wrongly. Open issue 20261028T075033Z-firehose-3-net-new-root-causes-the-resourcenotfoundexception.md carries the CreateDeliveryStream twin of this cause; StartDeliveryStreamEncryption is a separate call site and is not covered by it.
Fix: the StartDeliveryStreamEncryption handler's KMS validation step, where the KMS error is translated into InvalidKMSResourceException.
Traces: firehose.start-encryption-invalid-kms-key.

=== Cause 6 of 6: StartDeliveryStreamEncryption's ResourceNotFoundException reuses the control-plane-read sentence ("under account ... not found.") instead of its own ("under accountId ... not found") ===
AWS: Smithy: StartDeliveryStreamEncryption errors include ResourceNotFoundException (client, HTTP 400), "The specified resource could not be found." Recorded behavior: for a stream name that does not exist, this operation's message is `Firehose under accountId <account-id> not found` — fixed text `Firehose under accountId ` + the caller's 12-digit account id + ` not found`, with NO terminal period. The sibling control-plane read (DescribeDeliveryStream) words it differently, as `Firehose under account <account-id> not found.` — these two sentences are not interchangeable.
Emulator: StartDeliveryStreamEncryption answers the DescribeDeliveryStream sentence, `Firehose under account 000000000000 not found.` — "account" instead of "accountId" and an extra terminal period. (The account id itself is correct and is a value from the caller's identity, not part of the fixed text.)
Why: a client that string-matches or normalizes the not-found message per operation mis-handles the refusal, and a fixture recorded on AWS does not replay. Open issue 20261028T150444Z-firehose-stopdeliverystreamencryption-s-not-found-refusal-reuses-the.md describes exactly this defect for StopDeliveryStreamEncryption; the Start twin is a distinct call site and is reported here.
Fix: the StartDeliveryStreamEncryption handler's stream-lookup refusal path, which must build its own ResourceNotFoundException message rather than reusing the shared control-plane-read sentence.
Traces: firehose.start-encryption-nonexistent-stream-not-found.

## Evidence
Cause 1 — firehose.buffering-hint-interval-delays-delivery:
  assert not objs, "record delivered before buffer interval elapsed"
  AssertionError: record delivered before buffer interval elapsed: [{'Key': '<YYYY/MM/DD/HH>/<stream>-1-<ts>-0-<uuid>', 'Size': 14, ...}]
firehose.buffering-hint-size-triggers-early-flush:
  AssertionError: single below-threshold record was flushed early: [{'Key': '...', 'Size': 16, ...}]
  Expected: with IntervalInSeconds=60 (and a SizeInMBs threshold not reached), listing the destination shortly after PutRecord returns no objects.
  Actual: the object is already present, written in the same second as the put.

Cause 2 — firehose.server-side-assumes-configured-role-for-s3-put-object:
  AssertionError: record was delivered to S3 despite the delivery role's S3 permissions being revoked
firehose.server-side-role-permission-enforced-on-delivery:
  AssertionError: objects were delivered to S3 (2 > 1) after the delivery role's S3 write permission was revoked
  Expected: no new object after the role loses s3:PutObject.
  Actual: delivery continues unchanged.

Cause 3 — firehose.deploy-cfn-stream-delivers-to-s3: Error: Check timed out after 60.0s (polling the CFN-deployed stream's destination; no object).

Cause 4 — firehose.destination-retry-duration-respected: Error: Check timed out after 60.0s. The check first asserts the echoed config (RetryOptions.DurationInSeconds == 0, HECAcknowledgmentTimeoutInSeconds == 180, S3BackupMode == 'FailedEventsOnly'), then polls the backup bucket for a record whose base64 `rawData` contains the put marker and whose `errorCode` startswith 'Splunk.'. No backup object is ever produced.

Cause 5 — firehose.start-encryption-invalid-kms-key:
  assert message == expected
  AssertionError: expected message 'AccessDenied by KMS while using the CMK. Check the key policy to ensure that the KMS grant related apis are allowed. Try again after the key policy has the right permissions.', got "Key 'arn:aws:kms:<region>:<account>:key/<key-id>' does not exist"
  Fixed text (required): the whole AccessDenied-by-KMS sentence. Variable parts: none — the emulator's message wrongly interpolates the request's KMS key ARN.

Cause 6 — firehose.start-encryption-nonexistent-stream-not-found:
  assert message == expected
  AssertionError: expected message 'Firehose under accountId 000000000000 not found', got 'Firehose under account 000000000000 not found.'
  Fixed text (required): 'Firehose under accountId ' ... ' not found' with no terminal period. Variable part: the caller's 12-digit account id (000000000000 here is this environment's account, not part of the template).
  Error code and status are otherwise correct: ResourceNotFoundException / HTTP 400.

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Cause 1: CreateDeliveryStream (DirectPut, ExtendedS3DestinationConfiguration, BufferingHints {SizeInMBs: 1, IntervalInSeconds: 60}); wait ACTIVE; PutRecord a ~14-byte record; immediately ListObjectsV2 on the bucket — an object is already present. Repeat with a single record far below SizeInMBs.
3. Cause 2: create a stream whose RoleARN grants s3:PutObject on the bucket, PutRecord and confirm one object; then remove the role's S3 statement, PutRecord again and list — a second object appears.
4. Cause 3: deploy a CloudFormation stack containing AWS::KinesisFirehose::DeliveryStream with an S3 destination, wait CREATE_COMPLETE, PutRecord against the deployed stream, poll the bucket — nothing arrives.
5. Cause 4: CreateDeliveryStream with SplunkDestinationConfiguration HECEndpoint https://203.0.113.7:8088/services/collector/event, RetryOptions.DurationInSeconds=0, HECAcknowledgmentTimeoutInSeconds=180, S3BackupMode=FailedEventsOnly; PutRecord; poll the backup bucket — no object with rawData/errorCode is ever written.
6. Cause 5: StartDeliveryStreamEncryption on an ACTIVE stream with DeliveryStreamEncryptionConfigurationInput {KeyType: CUSTOMER_MANAGED_CMK, KeyARN: arn:aws:kms:<region>:<other-account>:key/<nonexistent-uuid>} and read Error.Message.
7. Cause 6: StartDeliveryStreamEncryption with DeliveryStreamName set to a stream that does not exist and read Error.Code/Error.Message.
