Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis
Issue-Key: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis|_poll_for#26|Timeout|time.sleep(interval_s)
Issue-Commit: 949e075d1963e2ebda7cf19988bf278a0e30c873

# kms__logs: a subscription filter on a CMK-encrypted log group delivers nothing to its Kinesis destination — the check now hangs to the 60 s cap instead of failing on a NotImplemented stub (1 cause)

> _Note: this issue was identified against the code base at commit `949e075d1963e2ebda7cf19988bf278a0e30c873`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: CloudWatch Logs subscription-filter fan-out from a CMK-encrypted log group to a Kinesis destination never produces a record, and the attempt no longer surfaces an API error — the check consumes the whole 60 s budget and is graded 'mismatch' with 'Check timed out after 60.0s' ===

AWS: PutSubscriptionFilter on a log group whose events are encrypted with a customer managed key installs a live fan-out to the destination stream; the CMK protects the events at rest inside CloudWatch Logs only. CWL decrypts service-side before delivery, so each record that lands on the Kinesis destination is a plain GZIP frame (the decompressed bytes begin with the gzip magic, not a KMS ciphertext blob) carrying a CWL envelope: messageType 'DATA_MESSAGE', logGroup equal to the encrypted group's name, logStream equal to the stream written to, subscriptionFilters equal to the single filter name, and logEvents whose messages are byte-for-byte and in-order what PutLogEvents accepted (including non-ASCII payload bytes). The group's kmsKeyId is unchanged before and after delivery, and the destination stream needs no encryption of its own for the payload to be readable. Smithy (com-amazonaws-cloudwatchlogs-logs-20140328): PutSubscriptionFilter input logGroupName/filterName/filterPattern/destinationArn all @required, roleArn optional, output Unit, modeled refusals InvalidParameterException / ResourceNotFoundException / InvalidOperationException / LimitExceededException / OperationAbortedException (all 400); PutLogEvents returns nextSequenceToken and reports partial refusals in rejectedLogEventsInfo. Nothing in the model describes the fan-out effect itself: recorded behavior only for the envelope/plaintext contract.

Emulator: every producer-side call is accepted (CreateKey with the logs-service key policy, CreateLogGroup with kmsKeyId, CreateLogStream, PutSubscriptionFilter with destinationArn/roleArn, PutLogEvents with no rejectedLogEventsInfo), but no record ever becomes readable on the destination stream, so the check's delivery poll (8 rounds of PutLogEvents, each polled for 45 s) never observes its marker and the run is cut off by the harness: 'Error: Check timed out after 60.0s'. No status code, error code or error message is produced anywhere in the flow — the divergence is a missing asynchronous effect, not a refusal. This is a THIRD distinct failure mode for this one check and neither already-open issue reproduces it any more: 20261024T150703Z-kms-logs-the-kinesis-createstream-stub-blocks-the.md is keyed on kms__logs|CreateStream|NotImplemented and 20261025T094106Z-kms-logs-the-kinesis-getsharditerator-stub-now-blocks.md on kms__logs|GetShardIterator|NotImplemented; this replay raises no NotImplemented at all, which is why the suite marked the check neither 'ALREADY FILED' nor 'OWNED by'. A fixer replaying either stored key would see the failure move rather than close.

Why: a client that builds a log-processing consumer on subscription-filter fan-out gets silence. Because the producer side all answers 200, there is no signal to distinguish the four possible emulator behaviors a consumer must care about — plaintext delivery (correct), ciphertext forwarded, records wrapped in a KMS envelope, or delivery withheld because the group is CMK-encrypted. A client waiting on delivery blocks until its own timeout rather than receiving records or an error, and any assertion that a CMK on the group does NOT leak into the destination payload is unverifiable here.

Fix: the CloudWatch Logs subscription-filter delivery path — the fan-out that PutLogEvents must drive for each installed filter on the group, including the service-side decrypt of a CMK-encrypted group, the GZIP-framed CWL envelope construction (messageType, logGroup, logStream, subscriptionFilters, logEvents) and the write to the Kinesis destination named by destinationArn — in the logs service (services/service-com-amazonaws-cloudwatchlogs-logs-20140328), together with the Kinesis data-plane read path it must land in (services/service-com-amazonaws-kinesis-kinesis-20131202, GetShardIterator/GetRecords). Note: kinesis is still in this project's PENDING bootstrap set, so part of this cause may be resolved by that bootstrap; the logs-side assertions (plaintext record, envelope naming the encrypted group and the filter, events verbatim and in order, group kmsKeyId unchanged after delivery) still need a re-replay afterwards, which is why the cause is recorded against the kms__logs group. Also note that the check's own internal waits (up to 180 s for the destination stream to reach ACTIVE, then up to 8 x 45 s of delivery polling) exceed the harness's 60 s per-check budget, so the exact point at which this run stalled is not pinned down by the report; a re-replay should confirm whether the stall is stream readiness or the absent delivery.

Traces: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis.

## Evidence
Failing check kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis, verdict mismatch:

  Error: Check timed out after 60.0s

Expected (AWS): after PutSubscriptionFilter(logGroupName=<CMK-encrypted group>, filterName=<f>, filterPattern='', destinationArn=<unencrypted Kinesis stream ARN>, roleArn=<logs delivery role>) and PutLogEvents of N events into stream 'app', GetShardIterator(TRIM_HORIZON) + GetRecords yield at least one record whose Data begins with the GZIP magic 0x1f 0x8b and whose decompressed JSON envelope has messageType 'DATA_MESSAGE', logGroup == the encrypted group name, logStream == 'app', subscriptionFilters == [<filter name>], and logEvents[].message equal, in order, to exactly the messages PutLogEvents accepted (the event marker appearing in the decompressed bytes as plaintext). DescribeLogGroups reports the same kmsKeyId before and after delivery, and DescribeStreamSummary reports EncryptionType 'NONE' for the destination throughout.

Actual (emulator): all producer-side calls are accepted and PutLogEvents reports no rejectedLogEventsInfo, but no record is ever readable on the destination stream; the delivery poll never satisfies its predicate and the check is terminated by the 60 s harness cap with no HTTP status, error code or error message emitted by any call. Unlike the two previously filed replays of this same check, no 'NotImplemented' is raised: the stored replay keys kms__logs|CreateStream|NotImplemented and kms__logs|GetShardIterator|NotImplemented no longer reproduce, and the replay report left this check unowned.

## Steps to Reproduce
1. sts:GetCallerIdentity to get the account id; kms:CreateKey with a key policy granting the account root kms:* and logs.<region>.amazonaws.com kms:Encrypt*/Decrypt*/ReEncrypt*/GenerateDataKey*/Describe* conditioned on ArnEquals kms:EncryptionContext:aws:logs:arn == arn:aws:logs:<region>:<account>:log-group:<group>.
2. kinesis:CreateStream(StreamName=<stream>, ShardCount=1, StreamMode=PROVISIONED); poll kinesis:DescribeStreamSummary until StreamStatus == ACTIVE and assert EncryptionType == 'NONE'; kinesis:ListShards for the shard ids.
3. iam:CreateRole trusting logs.amazonaws.com, with an inline policy allowing kinesis:PutRecord/PutRecords on the stream ARN.
4. logs:CreateLogGroup(logGroupName=<group>, kmsKeyId=<CMK ARN>); assert DescribeLogGroups reports that kmsKeyId; logs:CreateLogStream(logStreamName='app').
5. logs:PutSubscriptionFilter(logGroupName=<group>, filterName=<f>, filterPattern='', destinationArn=<stream ARN>, roleArn=<role ARN>) — accepted.
6. logs:PutLogEvents into 'app' with a few uniquely marked messages; assert no rejectedLogEventsInfo.
7. kinesis:GetShardIterator(ShardIteratorType='TRIM_HORIZON') then kinesis:GetRecords on each shard, repeatedly. AWS: records appear within the fan-out lag; each Data blob is plain GZIP whose envelope names the encrypted group, 'app' and [<f>], and carries the written messages verbatim. Emulator: no record ever appears and no call errors — the caller polls until its own timeout (here, the 60 s check cap).
8. logs:DescribeLogGroups again and confirm kmsKeyId is still the CMK.
