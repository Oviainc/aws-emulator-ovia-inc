Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis
Issue-Key: kms__logs|GetRecords|NotImplemented
Issue-Commit: 1fed27aa87ce52e7b35ebddc7461464a30baff91

# kms__logs: the CMK-encrypted-log-group subscription-filter plaintext contract is still ungraded — the blocking kinesis data-plane stub has moved from GetShardIterator to GetRecords

> _Note: this issue was identified against the code base at commit `1fed27aa87ce52e7b35ebddc7461464a30baff91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecords

## Description
One root cause. Cause count: 1.

Related open issues that do NOT cover this replay key: 20261024T150703Z-kms-logs-the-kinesis-createstream-stub-blocks-the.md carries key `kms__logs|CreateStream|NotImplemented` (CreateStream now succeeds) and 20261025T094106Z-kms-logs-the-kinesis-getsharditerator-stub-now-blocks.md carries key `kms__logs|GetShardIterator|NotImplemented` (GetShardIterator now succeeds). Neither key reproduces any more, which is why the replay report did not mark this check ALREADY FILED or OWNED. This issue records the key that reproduces TODAY so the check stays re-replayable; a fixer replaying only the two older keys would see the failure move again rather than close.

=== Cause 1 of 1: the Kinesis destination of a CloudWatch Logs subscription filter can be created and iterated, but its records still cannot be read, so the CMK-encrypted-log-group fan-out contract is never graded ===

AWS: CloudWatch Logs decrypts a CMK-encrypted log group's events service-side before subscription-filter fan-out, so what lands on the Kinesis destination is a plaintext, GZIP-framed CWL envelope: `messageType` "DATA_MESSAGE", `logGroup` naming the encrypted group, `logStream` naming the stream, `subscriptionFilters` a single-element list naming the filter, and `logEvents` byte-for-byte and in the order written by PutLogEvents. The group's CMK protects data at rest inside CloudWatch Logs only and does not propagate to the destination stream — the check creates the destination stream with EncryptionType NONE and asserts that, so the stream itself is not why the payload is readable. Reading those records is the standard Kinesis data-plane pair: GetShardIterator, then GetRecords returning a `Records` list whose `Data` member is the raw record blob (here, gzip magic 0x1f 0x8b followed by the JSON envelope). Recorded behavior only for the plaintext/envelope detail — no Smithy model for kinesis is present in the checkout to cite, because the service crate has not been stood up.

Emulator: kinesis GetRecords answers error code `NotImplemented` with message `operation GetRecords is scaffolded but not implemented in this emulator`. The fixed text of that message is `operation <OperationName> is scaffolded but not implemented in this emulator`; the only variable part is the operation name taken from the request (here GetRecords). AWS has no such error code for GetRecords at all — it returns 200 with a `Records` list (possibly empty) plus `NextShardIterator` and `MillisBehindLatest`. Because no record is ever read, none of the logs-side assertions execute. Everything before the read now succeeds: kinesis CreateStream, DescribeStreamSummary reporting EncryptionType NONE, ListShards, iam CreateRole/PutRolePolicy for the delivery role, logs CreateLogGroup with kmsKeyId set to the CMK ARN (and DescribeLogGroups echoing it), CreateLogStream, PutSubscriptionFilter, and PutLogEvents.

Why: a client cannot tell whether the emulator delivers a CMK-encrypted group's events as plaintext (correct), forwards ciphertext, wraps them in a KMS envelope, or silently withholds delivery from an encrypted group. Every log-processing consumer built on subscription-filter fan-out out of an encrypted group is unverifiable here, and a wrong answer would be silent, since the whole producer side returns success.

Fix: the kinesis data-plane read path — the GetRecords handler (the operation that feeds off GetShardIterator) in the kinesis service crate. Note that kinesis is still in this project's PENDING bootstrap set, so standing that service up may resolve the blocking call; the logs-side contract (plaintext GZIP records, envelope naming the encrypted group and the filter, logEvents verbatim and in order, group kmsKeyId unchanged before and after delivery) still requires a re-replay of this check afterwards, which is why the finding is recorded against the kms__logs group rather than left implicit in a bootstrap issue.

Traces: kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis.

## Evidence
Failing check kms__logs.xsvc-logs-2-subscription-filter-on-cmk-group-delivers-plaintext-to-kinesis, verdict error:

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetRecords operation: operation GetRecords is scaffolded but not implemented in this emulator

Expected (AWS): GetRecords on the iterator returned by GetShardIterator(ShardIteratorType='TRIM_HORIZON') answers HTTP 200 with a `Records` list plus `NextShardIterator` and `MillisBehindLatest`. Each record's `Data` begins with the GZIP magic bytes 0x1f 0x8b; decompressed it is a JSON CWL envelope with messageType 'DATA_MESSAGE', logGroup equal to the CMK-encrypted log group, logStream equal to the written stream, subscriptionFilters equal to a one-element list holding the filter name, and logEvents whose `message` values equal, in order, the events passed to PutLogEvents (the event marker appearing in the decompressed bytes in plaintext). DescribeLogGroups still reports the group's kmsKeyId as the CMK ARN after delivery.
Actual (emulator): error code `NotImplemented` from GetRecords, message `operation GetRecords is scaffolded but not implemented in this emulator`. No record is read, so no fan-out assertion runs.

Movement of the blocking call across runs (the reason this needs a fresh replay key):
  - previously recorded: kinesis CreateStream -> NotImplemented (issue 20261024T150703Z-kms-logs-the-kinesis-createstream-stub-blocks-the.md); CreateStream now succeeds.
  - then recorded: kinesis GetShardIterator -> NotImplemented (issue 20261025T094106Z-kms-logs-the-kinesis-getsharditerator-stub-now-blocks.md); GetShardIterator now succeeds.
  - this run: kinesis GetRecords -> NotImplemented. The check was NOT marked 'ALREADY FILED' or 'OWNED by' in the replay report, because no open issue carries the key kms__logs|GetRecords|NotImplemented.

## Steps to Reproduce
1. kms:CreateKey with a key policy granting logs.<region>.amazonaws.com kms:Encrypt*/Decrypt*/ReEncrypt*/GenerateDataKey*/Describe* conditioned on kms:EncryptionContext:aws:logs:arn equal to the log-group ARN.
2. kinesis:CreateStream(ShardCount=1, StreamMode=PROVISIONED) — succeeds; poll DescribeStreamSummary until ACTIVE and assert EncryptionType == 'NONE'; kinesis:ListShards returns the shard ids — succeeds.
3. iam:CreateRole trusting logs.amazonaws.com and iam:PutRolePolicy allowing kinesis:PutRecord/PutRecords on the stream ARN.
4. logs:CreateLogGroup(logGroupName=<group>, kmsKeyId=<CMK ARN>); logs:CreateLogStream(logStreamName='app'); confirm DescribeLogGroups reports kmsKeyId == <CMK ARN>.
5. logs:PutSubscriptionFilter(logGroupName=<group>, filterName=<f>, filterPattern='', destinationArn=<stream ARN>, roleArn=<role ARN>).
6. logs:PutLogEvents a small batch of events into 'app'.
7. kinesis:GetShardIterator(StreamName=<stream>, ShardId=<shard>, ShardIteratorType='TRIM_HORIZON') — succeeds and returns an iterator.
8. kinesis:GetRecords(ShardIterator=<it>, Limit=1000) — AWS returns 200 with the delivered records; the emulator answers error code NotImplemented. Then GZIP-decompress each record's Data and assert the CWL envelope names the encrypted group and the filter and carries the written log events verbatim in plaintext, and that the group's kmsKeyId is unchanged.
