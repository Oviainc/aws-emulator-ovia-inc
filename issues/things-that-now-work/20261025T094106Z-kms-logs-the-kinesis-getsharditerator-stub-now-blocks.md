Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis
Issue-Key: kms__logs|GetShardIterator|NotImplemented
Issue-Commit: ee3608f46372fd0499ccb380262b1cbf197ec697

# kms__logs: the kinesis GetShardIterator stub now blocks the CMK-encrypted-log-group subscription-filter plaintext check (the stale CreateStream replay key no longer reproduces it) (1 cause)

> _Note: this issue was identified against the code base at commit `ee3608f46372fd0499ccb380262b1cbf197ec697`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the Kinesis destination of a CloudWatch Logs subscription filter can now be created, but its records cannot be read, so the CMK-encrypted-log-group fan-out contract is still ungraded ===

AWS: CloudWatch Logs decrypts a CMK-encrypted log group's events service-side before subscription-filter fan-out, so what arrives at the Kinesis destination is a plaintext (GZIP-framed) CWL envelope — messageType DATA_MESSAGE, logGroup naming the encrypted group, logStream, subscriptionFilters naming the filter, and logEvents byte-for-byte as written. The group's CMK protects data at rest in CWL only and does not propagate to the destination stream (the check creates the destination stream with EncryptionType NONE and asserts that, so the stream is not why the payload is readable). Reading those records requires the Kinesis data plane: GetShardIterator then GetRecords. Recorded behavior only for the plaintext/envelope detail; the Kinesis read path itself is the standard GetShardIterator -> GetRecords pair.

Emulator: kinesis GetShardIterator answers error code NotImplemented ('operation GetShardIterator is scaffolded but not implemented in this emulator'), so no record can be read off the destination stream and the logs-side assertions are never reached. This is a DIFFERENT blocking operation than the one previously recorded for this check: the earlier blocker, kinesis CreateStream, now succeeds (the check gets past CreateStream, DescribeStreamSummary reporting EncryptionType NONE, and ListShards), so the open issue 20261024T150703Z-kms-logs-the-kinesis-createstream-stub-blocks-the.md carries a replay key (kms__logs|CreateStream|NotImplemented) that no longer reproduces, and the replay report consequently did not mark this check as OWNED by it. This issue records the current key so the check is re-replayable.

Why: a client cannot tell whether the emulator delivers a CMK-encrypted group's events as plaintext (correct), forwards ciphertext, wraps them in a KMS envelope, or simply withholds delivery from an encrypted group. Any log-processing consumer built against subscription-filter fan-out is unverifiable here, and a wrong answer would be silent — the producer side (CreateLogGroup with kmsKeyId, PutSubscriptionFilter, PutLogEvents) all succeeds.

Fix: the kinesis data-plane read path — GetShardIterator (and the GetRecords it feeds) in services/service-com-amazonaws-kinesis-kinesis-20131202. Note the kinesis service is still in this project's PENDING bootstrap set, so its bootstrap may resolve the blocking call; the logs-side assertion (plaintext records, envelope naming the encrypted group and the filter, group kmsKeyId unchanged before and after delivery) still needs a re-replay afterwards, which is why this is recorded against the kms__logs group rather than left implicit.

Traces: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis.

## Evidence
Failing check kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis, verdict error:

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetShardIterator operation: operation GetShardIterator is scaffolded but not implemented in this emulator

Progress since the previously recorded blocker: the check now gets through kinesis CreateStream, describe_stream_summary (asserting EncryptionType == 'NONE'), list_shards, iam role creation, logs:CreateLogGroup(kmsKeyId=<CMK ARN>) with the group's kmsKeyId asserted equal to the key ARN, CreateLogStream, and PutSubscriptionFilter(destinationArn=<stream ARN>, roleArn=...). It then fails on the first Kinesis read.

Expected (AWS): GetShardIterator returns a shard iterator; GetRecords then yields records whose GZIP-decompressed body is a plaintext CWL envelope with messageType 'DATA_MESSAGE', logGroup == the CMK-encrypted group, logStream == 'app', subscriptionFilters == [<filter name>], and logEvents equal in order to the events written by PutLogEvents (the event marker appearing in the decompressed bytes in plaintext), with the group's kmsKeyId still the CMK after delivery.
Actual (emulator): HTTP error code NotImplemented from GetShardIterator; no record is ever read, so none of the fan-out assertions execute.

This check was NOT marked 'ALREADY FILED' or 'OWNED by' in the replay report. The open issue 20261024T150703Z-kms-logs-the-kinesis-createstream-stub-blocks-the.md names the same trace id but its Issue-Key is 'kms__logs|CreateStream|NotImplemented', which no longer reproduces now that CreateStream works; a fixer replaying only that key would see the failure move rather than close.

## Steps to Reproduce
1. kms:CreateKey with a key policy granting logs.<region>.amazonaws.com the encrypt/describe permissions for the log-group ARN.
2. kinesis:CreateStream(ShardCount=1) — succeeds now; kinesis:DescribeStreamSummary reports EncryptionType 'NONE'; kinesis:ListShards returns the shard ids.
3. iam:CreateRole for CloudWatch Logs delivery to that stream.
4. logs:CreateLogGroup(logGroupName=<group>, kmsKeyId=<CMK ARN>); logs:CreateLogStream(logStreamName='app'); confirm DescribeLogGroups reports the kmsKeyId.
5. logs:PutSubscriptionFilter(logGroupName=<group>, filterName=<f>, filterPattern='', destinationArn=<stream ARN>, roleArn=<role>).
6. logs:PutLogEvents a few events into 'app'.
7. kinesis:GetShardIterator(StreamName=<stream>, ShardId=<shard>, ShardIteratorType='TRIM_HORIZON') — AWS returns an iterator; the emulator answers NotImplemented. (Then kinesis:GetRecords, GZIP-decompress each record and assert the CWL envelope names the encrypted group and the filter and carries the written log events verbatim in plaintext.)
