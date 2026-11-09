Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-without-partition-spec-rejected
Issue-Key: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table|_wait_for_table#13|AssertionError|raise AssertionError(
Issue-Key: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-without-partition-spec-rejected|_create_channel_expect_error#3|AssertionError|raise AssertionError(
Issue-Commit: e0dfe85212dfb0c22b04d41b86c8cbe201f3bba8

# Kinesis CreateChannel with an S3 Tables destination never creates the destination Iceberg table, and accepts an S3TablesConfiguration with no PartitionSpec

> _Note: this issue was identified against the code base at commit `e0dfe85212dfb0c22b04d41b86c8cbe201f3bba8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Two net-new root causes in Kinesis CreateChannel's S3 Tables (streaming-table) destination path.

Cause 1 of 2: a channel created with S3TablesDestinationConfiguration reaches ChannelStatus ACTIVE and echoes its S3TablesConfigurationList correctly, but the destination Iceberg table is never created in the target table bucket/namespace, so the table never appears to S3 Tables.
AWS: recorded behavior only for the provisioning side effect (Smithy: CreateChannel input S3TablesDestinationConfiguration -> S3TablesConfigurationList of S3TablesConfiguration{TableBucketARN, Namespace, TableName, CompressionType, PartitionSpec}; the TableName member is documented "The name of the destination table. Amazon Kinesis Data Streams creates this table in the specified table bucket."). Once the channel is ACTIVE and records flow, S3 Tables holds a table whose name is the configured TableName, whose namespace is the single-element list [<configured Namespace>], and whose managedByService is "kinesis.amazonaws.com" (Smithy s3tables: GetTable output members name, namespace, managedByService), and ListTables on that bucket/namespace reports exactly that table.
Emulator: the channel goes ACTIVE but no table is ever created — the check polls GetTable/ListTables for 720 s and the table never appears: "channel <channel-ARN> never created table '<table-name>' within 720.0s".
Why: the whole streaming-table delivery feature is unobservable — a client that creates a channel and then reads or queries the Iceberg table it was promised finds nothing, and nothing ever reports managedByService, so no caller can tell a Kinesis-managed table from a user table.
Fix: the CreateChannel handler's S3 Tables destination provisioning (it must create the configured table in the S3 Tables state, marked managed by kinesis.amazonaws.com), and the S3 Tables table state it writes into.
Failing check ids: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table.

Cause 2 of 2: CreateChannel with an S3TablesConfiguration that omits PartitionSpec is accepted and returns a ChannelARN instead of being refused.
AWS: Smithy models PartitionSpec as an optional member of S3TablesConfiguration (com.amazonaws.kinesis#S3TablesConfiguration.PartitionSpec -> PartitionSpec{PartitionFields, 1..10, required}), so the model does not state the rule; the recorded behavior is that the service refuses the request — HTTP 400, error code ValidationException (Smithy: CreateChannel errors include ValidationException, httpError 400), with the message "PartitionSpec is required." That message is entirely fixed text with no variable part, and the rejected attempt leaves no channel on the stream. The same request with PartitionSpec {"PartitionFields": [{"Transform": "TIME_HOUR", "SourceName": "<field>"}]} is accepted and reaches ACTIVE.
Emulator: the call succeeds and returns a channel ARN — "CreateChannel with no PartitionSpec unexpectedly succeeded: 'arn:aws:kinesis:<region>:<account>:channel/<channel-name>'" — so the modelled refusal class is never reached and an unpartitioned channel is left registered on the stream.
Why: a client that relies on the service to reject an incomplete streaming-table configuration instead gets a channel that cannot deliver correctly, and the misconfiguration surfaces only later (or never); error-handling code paths keyed on ValidationException / "PartitionSpec is required." never run.
Fix: CreateChannel's request validation for the S3 Tables destination (cross-field requirement of PartitionSpec whenever S3TablesDestinationConfiguration is present), ahead of any channel being persisted.
Failing check ids: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-without-partition-spec-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.
Cause 1 — [mismatch] glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table, line 55 `table = _wait_for_table(ctx, infra, channel_arn)` -> line 398 AssertionError: "channel arn:aws:kinesis:us-west-2:000000000000:channel/sim2real-xsvc-stbl-ch-<id> never created table 'sim2real_xsvc_stbl_table_<id>' within 720.0s". The checks before it passed: ChannelStatus became ACTIVE, S3TablesDestinationConfiguration.S3TablesConfigurationList[0].TableBucketARN and .TableName matched the request, and the summary's ChannelDestinationType was S3_TABLES. Expected: GetTable/ListTables on the configured table bucket + namespace report the configured TableName with namespace [<namespace>] and managedByService "kinesis.amazonaws.com". The channel/table/bucket names and the account in the message are per-run values, not part of the requirement.
Cause 2 — [mismatch] glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-without-partition-spec-rejected, line 28 -> line 291 AssertionError: "CreateChannel with no PartitionSpec unexpectedly succeeded: 'arn:aws:kinesis:us-west-2:000000000000:channel/sim2real-xsvc-pspec-bad-<id>'". Expected (code, message) == ("ValidationException", "PartitionSpec is required.") with HTTP 400 and no channel left on the stream; the emulator returned 200 with a ChannelARN.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a Kinesis stream, an S3 Tables table bucket + namespace, an S3 DLQ bucket and the service-execution IAM role.
3. Cause 2: call kinesis.create_channel(ChannelName=<name>, ServiceExecutionRoleARN=<role>, StreamConfigurationList=[...], S3TablesDestinationConfiguration={"DeadLetterQueueS3Configuration": {...}, "S3TablesConfigurationList": [{"TableBucketARN": <bucket-arn>, "Namespace": <ns>, "TableName": <table>, "CompressionType": "ZSTD"}]}) — i.e. with PartitionSpec omitted. Expect HTTP 400 ValidationException "PartitionSpec is required." and ListChannels on the stream to stay empty; the emulator returns a ChannelARN.
4. Cause 1: repeat the same call WITH PartitionSpec {"PartitionFields": [{"Transform": "TIME_HOUR", "SourceName": <field>}]}, wait for ChannelStatus ACTIVE, put records on the stream, then poll s3tables.get_table / list_tables(tableBucketARN=<bucket-arn>, namespace=<ns>). Expect the configured table to appear with namespace [<ns>] and managedByService "kinesis.amazonaws.com"; the emulator never creates it.
