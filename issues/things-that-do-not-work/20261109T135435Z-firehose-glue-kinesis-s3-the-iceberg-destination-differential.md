Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__kinesis__s3.kds-an-unmarked-destination-table-swallows-records-a-marked-one-commits
Issue-Key: firehose__glue__kinesis__s3.kds-an-unmarked-destination-table-swallows-records-a-marked-one-commits|_poll#18|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# firehose__glue__kinesis__s3: the Iceberg destination differential never reaches a verdict — neither the marked nor the unmarked Glue table receives any delivered data

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
=== Cause 1 of 1: an Iceberg (Glue-table) destination writes no data and no committed snapshot, so the marked/unmarked table differential cannot be graded ===
Failing check: firehose__glue__kinesis__s3.kds-an-unmarked-destination-table-swallows-records-a-marked-one-commits.
AWS: a Firehose stream whose destination is an Apache Iceberg table (IcebergDestinationConfiguration naming a Glue CatalogConfiguration/DestinationTableConfiguration) delivers records into that table's data location and commits them: after the buffering interval the table's warehouse prefix contains data objects and a new metadata.json whose current snapshot includes them, so the records are readable through the table. The differential the check grades is that a destination Glue table NOT marked as Iceberg (no table_type=ICEBERG / no metadata_location) does not become a committed Iceberg table, while a properly marked one commits — a contract that is only meaningful if the marked leg actually commits.
Emulator: nothing is delivered on either leg; the check reached the harness ceiling with no verdict — "Check timed out after 900.0s" (its own delivery deadline is 900 s, measured at ~90 s to the first object on real AWS). So the positive leg never produces data objects or a snapshot, and the negative leg is vacuously "satisfied".
Why: a client cannot test an Iceberg ingestion pipeline at all: no data objects, no metadata.json, no snapshot, and no distinction between a correctly marked destination table and one that AWS would refuse to commit into. Any test asserting "the unmarked table swallows records" passes for the wrong reason, so the boundary is ungradeable.
Fix: the Iceberg delivery path of the firehose service crate must write the buffered records as data files under the destination table's location and commit them (new metadata.json + snapshot, updated metadata_location on the Glue table), and must distinguish a destination table carrying the Iceberg marker from one that does not. Related open issues 20261028T122732Z-firehose-glue-kinesis-s3-the-apache-iceberg-path.md and 20261107T185637Z-firehose-glue-kinesis-s3-a-firehose-iceberg-destination-writes-no-table-metadata.md cover neighbouring Iceberg symptoms, but the replay does not mark this check as owned by either, so neither carries its replay key.
Traces: the check id above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — firehose__glue__kinesis__s3 9/23 (39%), 14 failing checks of which this one is marked neither ALREADY FILED nor OWNED.

Expected (AWS): within the check's 900 s delivery deadline (real-AWS measurement in the check: ~90.3 s to the first delivered object with 60 s / 1 MiB hints) the Iceberg-marked destination table's prefix holds data objects plus a metadata.json whose current snapshot lists them, while the unmarked destination table does not commit the records.
Actual (emulator): "Check timed out after 900.0s" — the check never observed any delivered object or committed snapshot on either leg and reached no verdict.

## Steps to Reproduce
1. Boot the emulator and point botocore at it. s3.create_bucket(Bucket=B).
2. glue.create_database(...); glue.create_table(T_marked, ...) with the Iceberg marker (table_type=ICEBERG and a metadata_location under s3://B/...) and glue.create_table(T_unmarked, ...) with the same columns and NO Iceberg marker.
3. kinesis.create_stream(...); iam.create_role(R) trusted by firehose.amazonaws.com with kinesis read, glue read/update and S3 write on B.
4. firehose.create_delivery_stream(DeliveryStreamType='KinesisStreamAsSource', KinesisStreamSourceConfiguration={...}, IcebergDestinationConfiguration={'RoleARN': R, 'CatalogConfiguration': {...}, 'DestinationTableConfigurationList': [{'DestinationDatabaseName': DB, 'DestinationTableName': T_marked}], 'S3Configuration': {...}, 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}) and a sibling stream targeting T_unmarked; wait ACTIVE.
5. kinesis.put_records of a handful of JSON records into each source stream.
6. Poll s3.list_objects_v2(Bucket=B, Prefix=<table location>) and glue.get_table for the updated metadata_location for up to 900 s.
7. Expect data objects and a new committed snapshot for the marked table and no commit for the unmarked one; observe no objects and no metadata for either, with the check hitting its 900 s cap.
