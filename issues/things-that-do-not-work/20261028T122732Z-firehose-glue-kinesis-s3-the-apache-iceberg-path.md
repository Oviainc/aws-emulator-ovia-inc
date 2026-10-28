Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__kinesis__s3.kds-create-delivery-stream-accepts-a-glue-table-with-no-iceberg-marker firehose__glue__kinesis__s3.kds-create-delivery-stream-rejects-a-destination-database-that-does-not-exist firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location firehose__glue__kinesis__s3.kds-records-put-to-kds-land-as-iceberg-data-and-a-committed-snapshot firehose__glue__kinesis__s3.kds-encryption-configuration-is-reported-for-directput-and-omitted-for-a-kds-source
Issue-Key: firehose__glue__kinesis__s3.kds-create-delivery-stream-accepts-a-glue-table-with-no-iceberg-marker|_error#23|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: firehose__glue__kinesis__s3.kds-create-delivery-stream-rejects-a-destination-database-that-does-not-exist|_error#14|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: firehose__glue__kinesis__s3.kds-records-put-to-kds-land-as-iceberg-data-and-a-committed-snapshot|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: firehose__glue__kinesis__s3.kds-encryption-configuration-is-reported-for-directput-and-omitted-for-a-kds-source|run#18|AssertionError|assert "DeliveryStreamEncryptionConfiguration" not in sourced_desc, (
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# firehose__glue__kinesis__s3: the Apache Iceberg path is unvalidated and inert — CreateDeliveryStream accepts an absent destination table/database, Glue IcebergInput writes no metadata.json, Firehose delivers no data or snapshot, and DescribeDeliveryStream reports encryption for a KDS-sourced stream (4 causes)

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
Four net-new root causes on the Firehose Iceberg-destination path and the Glue Iceberg table marker it depends on. No open issue covers them: 20261024T005347Z-firehose-glue-s3-... and 20261025T065545Z-firehose-glue-s3-a-delivery-stream-with-dataformatconversionconfiguration.md cover the Parquet DataFormatConversionConfiguration path on an S3 destination, not the Iceberg destination, its catalog validation or its snapshot commit.

Cause 1 of 4: CreateDeliveryStream does not validate that the Iceberg destination's Glue table/database exists, so it accepts a stream that can never deliver.
AWS: Smithy: CreateDeliveryStream takes IcebergDestinationConfiguration (CatalogConfiguration.CatalogARN, DestinationTableConfigurationList of {DestinationDatabaseName, DestinationTableName}, RoleARN) and is modeled with InvalidArgumentException (client, HTTP 400). Real Firehose performs a glue:GetTable with the supplied RoleARN at create time and refuses a destination whose table — or whose DATABASE — does not exist, with InvalidArgumentException whose message is the fixed text `Role <RoleARN> is not authorized to perform: glue:GetTable for the given table or the table does not exist.`; the only variable part is the RoleARN taken from the request (AWS deliberately conflates 'absent' with 'not authorized', so an absent DATABASE yields the same table-worded message). A table that merely lacks an Iceberg marker IS accepted and reaches ACTIVE (that half of the check passed).
Emulator: both creates SUCCEED (HTTP 200, a stream is minted) — no glue:GetTable existence check happens at all, for either an absent table name or an absent database name.
Why: the caller loses the only create-time signal that its catalog target is wrong; a stream goes ACTIVE and then silently delivers nowhere, and an absent-database typo is never reported.
Fix: the CreateDeliveryStream handler's Iceberg destination validation (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/create_delivery_stream.rs) must resolve every DestinationTableConfigurationList entry against the Glue catalog before accepting, and raise InvalidArgumentException with AWS's text and the request's RoleARN; a refused create must leave no delivery stream behind.
Traces: firehose__glue__kinesis__s3.kds-create-delivery-stream-accepts-a-glue-table-with-no-iceberg-marker (check line 285), firehose__glue__kinesis__s3.kds-create-delivery-stream-rejects-a-destination-database-that-does-not-exist (check line 258).

Cause 2 of 4: Glue CreateTable with OpenTableFormatInput.IcebergInput back-fills metadata_location but never writes the metadata.json object it points at.
AWS: recorded behavior only (the Smithy model declares OpenTableFormatInput.IcebergInput{MetadataOperation=CREATE, Version} on CreateTable but not the side effect): real Glue back-fills Parameters['table_type']='ICEBERG' and Parameters['metadata_location'] to an s3:// URI under the table's StorageDescriptor.Location ending in '.metadata.json', AND writes exactly one such object under <Location>metadata/ whose body has format-version matching IcebergInput.Version, 'location' equal to the table Location without its trailing slash, a schema whose fields are the StorageDescriptor Columns in order, and an empty 'snapshots' list.
Emulator: the catalog parameters are back-filled (those assertions passed, so metadata_location exists and is well-formed), but no object ever appears under the iceberg/<table>/metadata/ prefix in the bucket — the check exhausted its poll waiting for the key named by metadata_location.
Why: metadata_location points at a nonexistent object, so every Iceberg reader (Athena, Spark, Firehose) that resolves the pointer fails; the catalog and storage disagree.
Fix: the CreateTable handler's OpenTableFormatInput/IcebergInput branch (services/service-com-amazonaws-glue-awsglue/src/ops/create_table.rs) must materialize the format-version-2 metadata.json into S3 at the location it records.
Traces: firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location.

Cause 3 of 4: an Iceberg-destination delivery stream delivers nothing — no data objects, no manifest/snapshot, and metadata_location never advances.
AWS: recorded behavior only: with a KinesisStreamAsSource stream and BufferingHints{SizeInMBs:1, IntervalInSeconds:60}, records put to the source Kinesis stream land under <table Location>data/ as .parquet objects, one manifest-list snap-*.avro and one *-m0.avro manifest appear under metadata/, a second NNNNN.metadata.json version is written, Glue's Parameters['metadata_location'] advances to that newest version, and its single snapshot's summary['added-records'] equals the number of records produced.
Emulator: nothing is delivered. The stream reaches ACTIVE and kinesis:PutRecords reports FailedRecordCount 0, but the poll for any object under iceberg/<table>/data/ plus any .avro never satisfies and the check is killed at its time budget; consistent with the already-filed finding that the Firehose delivery pipeline writes no objects on the Parquet path.
Why: an Iceberg lakehouse pipeline silently loses every record — producers see success, the table stays empty, and no snapshot is ever committed, so this cannot be exercised end to end at all.
Fix: the Firehose delivery engine's Iceberg destination writer and its Glue commit step (services/service-com-amazonaws-firehose-firehose-20150804/src/ — the delivery/destination processing for IcebergDestinationConfiguration) must buffer source records, write Parquet data objects, commit a snapshot (manifest + manifest list + new metadata.json) and UpdateTable the catalog's metadata_location.
Traces: firehose__glue__kinesis__s3.kds-records-put-to-kds-land-as-iceberg-data-and-a-committed-snapshot.

Cause 4 of 4: DescribeDeliveryStream reports DeliveryStreamEncryptionConfiguration on a KinesisStreamAsSource stream, which AWS omits.
AWS: Smithy: DescribeDeliveryStream returns DeliveryStreamDescription with an OPTIONAL DeliveryStreamEncryptionConfiguration member. Server-side encryption (StartDeliveryStreamEncryption/DeliveryStreamEncryptionConfigurationInput) applies only to the direct-put path, so for a stream whose DeliveryStreamType is KinesisStreamAsSource real Firehose omits the DeliveryStreamEncryptionConfiguration key entirely from the description; it is present (e.g. Status DISABLED) only for a DirectPut stream.
Emulator: the KinesisStreamAsSource stream's description carries DeliveryStreamEncryptionConfiguration {'Status': 'DISABLED'}.
Why: a client reading the description cannot tell the two source families apart, and code that treats the key's presence as 'SSE is manageable on this stream' will call StartDeliveryStreamEncryption on a stream that cannot accept it.
Fix: the DescribeDeliveryStream serializer / the delivery-stream state that records encryption (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/describe_delivery_stream.rs) must leave the member unset for a KDS-sourced stream instead of defaulting it to DISABLED.
Traces: firehose__glue__kinesis__s3.kds-encryption-configuration-is-reported-for-directput-and-omitted-for-a-kds-source (check line 284).

## Evidence
Cause 1 — kds-create-delivery-stream-accepts-a-glue-table-with-no-iceberg-marker / kds-create-delivery-stream-rejects-a-destination-database-that-does-not-exist:
  expected: CreateDeliveryStream -> InvalidArgumentException (HTTP 400), Message 'Role <RoleARN> is not authorized to perform: glue:GetTable for the given table or the table does not exist.' (fixed text; RoleARN from the request) and no stream left behind
  actual:   HTTP 200, the delivery stream is created
  (AssertionError: the call succeeded; real AWS rejects it, so there is no error code — raised at check lines 285 and 258 respectively)

Cause 2 — kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location:
  expected: exactly one object at the key named by Glue Parameters['metadata_location'] under iceberg/<table>/metadata/, body with format-version 2, location = the table Location, fields = [id, data, timestamp], snapshots = []
  actual:   Parameters['table_type']='ICEBERG' and a well-formed metadata_location are returned, but the metadata/ prefix stays empty; the poll for that key never satisfies (check killed at its 60 s budget while polling)

Cause 3 — kds-records-put-to-kds-land-as-iceberg-data-and-a-committed-snapshot:
  expected: after kinesis:PutRecords of 8 records (FailedRecordCount 0) into the source stream of an ACTIVE Iceberg-destination stream: .parquet objects under iceberg/<table>/data/, one snap-*.avro, one *-m0.avro, two *.metadata.json versions, Glue metadata_location advanced to the newest, one snapshot with summary['added-records'] = 8
  actual:   no object of any kind appears under iceberg/<table>/; the delivery poll never satisfies (check killed at its 60 s budget)

Cause 4 — kds-encryption-configuration-is-reported-for-directput-and-omitted-for-a-kds-source:
  expected: DeliveryStreamDescription of a KinesisStreamAsSource stream contains NO 'DeliveryStreamEncryptionConfiguration' key
  actual:   DeliveryStreamEncryptionConfiguration {'Status': 'DISABLED'}
  (AssertionError: the KinesisStreamAsSource stream reports DeliveryStreamEncryptionConfiguration {'Status': 'DISABLED'}; real Firehose omits the key for a KDS-sourced stream)

## Steps to Reproduce
Shared setup: create an S3 bucket, an IAM role trusted by firehose.amazonaws.com with s3/kinesis/glue(GetTable,GetTableVersion,GetTableVersions,GetDatabase,UpdateTable)/logs permissions, a Glue database, and a Glue table (TableType EXTERNAL_TABLE, Location s3://<bucket>/iceberg/<table>/, columns id:string, data:string, timestamp:timestamp) created with OpenTableFormatInput={'IcebergInput':{'MetadataOperation':'CREATE','Version':'2'}}. CatalogARN = arn:aws:glue:<region>:<account>:catalog.

Cause 1: call CreateDeliveryStream(DeliveryStreamType='DirectPut', IcebergDestinationConfiguration with DestinationTableConfigurationList=[{DestinationDatabaseName:<real db>, DestinationTableName:<a table name that does not exist>}]) and again with [{DestinationDatabaseName:<a database that does not exist>, DestinationTableName:<real table>}]. Each must fail InvalidArgumentException with the glue:GetTable message and must not appear in ListDeliveryStreams.

Cause 2: after the shared CreateTable above, GetTable and read Parameters['metadata_location']; then list the bucket under iceberg/<table>/metadata/ and GetObject that exact key — it must exist exactly once and parse as format-version 2 Iceberg metadata with empty snapshots.

Cause 3: create a Kinesis stream, then CreateDeliveryStream(DeliveryStreamType='KinesisStreamAsSource', KinesisStreamSourceConfiguration={KinesisStreamARN, RoleARN}, IcebergDestinationConfiguration=... BufferingHints={'SizeInMBs':1,'IntervalInSeconds':60}); wait for ACTIVE; kinesis:PutRecords 8 JSON records; list iceberg/<table>/ and re-read the Glue table's metadata_location.

Cause 4: create one DirectPut Iceberg stream and one KinesisStreamAsSource Iceberg stream; DescribeDeliveryStream both and compare presence of the DeliveryStreamEncryptionConfiguration key.
