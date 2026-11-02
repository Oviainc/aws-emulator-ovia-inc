Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-kfi-traj-directput-silently-drops-kinesis-source
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-kfi-traj-directput-silently-drops-kinesis-source firehose-kfi-traj-firehose-role-mistakes firehose-kfi-traj-fix-table-via-update-destination firehose-kfi-traj-iceberg-config-required-members firehose-kfi-traj-kinesis-source-mistakes firehose-kfi-traj-stream-before-glue-table firehose-kfi-traj-rerun-reference-script
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].IcebergDestinationDescription.AppendOnly
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].IcebergDestinationDescription.BufferingHints
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].IcebergDestinationDescription.RetryOptions
Issue-Key: firehose|DescribeDeliveryStream|2xx->2xx|$.DeliveryStreamDescription.Destinations[*].IcebergDestinationDescription.S3BackupMode
Issue-Commit: 81fc6333f2dd5c26c146222a43c6f73baa469449

# firehose: IcebergDestinationDescription reports no materialised defaults for AppendOnly, BufferingHints, RetryOptions and S3BackupMode (2 causes)

> _Note: this issue was identified against the code base at commit `81fc6333f2dd5c26c146222a43c6f73baa469449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
Two net-new wire causes, both the same defect in how an Iceberg destination is rendered back by DescribeDeliveryStream: the description passes the submitted configuration through verbatim instead of reporting the destination's EFFECTIVE configuration with Firehose's documented defaults materialised, the way every other destination kind in this emulator does. No open issue carries these replay keys (replay reported owned_by empty for both). The open behavioral issue 20261028T122732Z-firehose-glue-kinesis-s3-the-apache-iceberg-path.md concerns Iceberg catalog validation, delivery and the encryption member of the description, not these four omitted members.

Cause 1 of 2: BufferingHints, RetryOptions, S3BackupMode and AppendOnly are all absent from IcebergDestinationDescription when the create left them out.
AWS: Smithy: IcebergDestinationDescription declares BufferingHints, RetryOptions, S3BackupMode (enum IcebergS3BackupMode — 'Currently, Firehose only supports FailedDataOnly') and AppendOnly (BooleanObject) as optional members; BufferingHints$SizeInMBs documents 'The default value is 5' and BufferingHints$IntervalInSeconds 'The default value is 300', and RetryOptions$DurationInSeconds documents the 300-second default. The recording shows AWS reporting all four for a stream created WITHOUT them: BufferingHints{IntervalInSeconds 300, SizeInMBs 5}, RetryOptions{DurationInSeconds 300}, S3BackupMode 'FailedDataOnly' and AppendOnly false — i.e. the description is the effective configuration, with the service's defaults filled in.
Emulator: all four members are missing from $.DeliveryStreamDescription.Destinations[0].IcebergDestinationDescription. The Iceberg arm of the destination renderer clones the stored configuration's optional members straight through (services/service-com-amazonaws-firehose-firehose-20150804/src/ops/describe_delivery_stream.rs, destination_description(), Kind::Iceberg at lines 477-495: buffering_hints/retry_options/s3_backup_mode/append_only are plain .clone()s), whereas the ExtendedS3, Redshift, Elasticsearch and Splunk arms in the same function route through buffering_hints()/s3_backup_mode() with the DEFAULT_* constants declared at lines 237-255.
Why: a client that reads its configuration back — to diff desired against actual, to re-submit it through UpdateDestination, or to assert on the buffering interval it will be charged for — sees four unset members and cannot tell 'the service applied its default' from 'this destination has no buffering/retry/backup policy at all'. SDK-typed clients get None/null where AWS gives a value, so any non-optional consumption of those fields breaks.
Fix: the Iceberg arm of destination_description() in src/ops/describe_delivery_stream.rs must materialise the documented defaults for BufferingHints (300 s / 5 MB), RetryOptions (300 s), S3BackupMode (FailedDataOnly) and AppendOnly (false) exactly as the other destination arms do, reporting the supplied value when the create named one.
Traces: firehose-kfi-traj-directput-silently-drops-kinesis-source#task#14 (representative; the cluster covers 8 steps across firehose-kfi-traj-firehose-role-mistakes, firehose-kfi-traj-fix-table-via-update-destination, firehose-kfi-traj-iceberg-config-required-members, firehose-kfi-traj-kinesis-source-mistakes and firehose-kfi-traj-stream-before-glue-table).

Cause 2 of 2: the same omission with BufferingHints supplied — AppendOnly, RetryOptions and S3BackupMode are still missing.
AWS: as above; here the create DID carry BufferingHints (so that member matches), and AWS still reports AppendOnly false, RetryOptions{DurationInSeconds 300} and S3BackupMode 'FailedDataOnly'.
Emulator: those three members are missing from the Iceberg destination description.
Why: same as cause 1 — it confirms the defect is per-member default materialisation and not a wholesale failure to render the block, so a fix that only defaults BufferingHints leaves this replay key failing.
Fix: same site; this cause exists to pin the three members that are independent of BufferingHints.
Traces: firehose-kfi-traj-rerun-reference-script#task#13.

## Evidence
Cause 1 — fixture firehose-kfi-traj-directput-silently-drops-kinesis-source step task#14 (DescribeDeliveryStream, HTTP 200 on both sides):
  $.DeliveryStreamDescription.Destinations[0].IcebergDestinationDescription.AppendOnly:     expected false,                                      actual missing
  ...IcebergDestinationDescription.BufferingHints:  expected {IntervalInSeconds: 300, SizeInMBs: 5},     actual missing
  ...IcebergDestinationDescription.RetryOptions:    expected {DurationInSeconds: 300},                   actual missing
  ...IcebergDestinationDescription.S3BackupMode:    expected 'FailedDataOnly',                           actual missing
  (affected_count 8: firehose-kfi-traj-firehose-role-mistakes#task#18, firehose-kfi-traj-fix-table-via-update-destination#task#10/#task#12/#task#15, firehose-kfi-traj-iceberg-config-required-members#task#13, firehose-kfi-traj-kinesis-source-mistakes#task#13, firehose-kfi-traj-stream-before-glue-table#task#13)

Cause 2 — fixture firehose-kfi-traj-rerun-reference-script step task#13 (DescribeDeliveryStream, HTTP 200 on both sides):
  ...IcebergDestinationDescription.AppendOnly:   expected false,                     actual missing
  ...IcebergDestinationDescription.RetryOptions: expected {DurationInSeconds: 300},   actual missing
  ...IcebergDestinationDescription.S3BackupMode: expected 'FailedDataOnly',           actual missing

Code: services/service-com-amazonaws-firehose-firehose-20150804/src/ops/describe_delivery_stream.rs lines 477-495 (Kind::Iceberg arm, plain clones) versus lines 237-255 (DEFAULT_BUFFERING_SIZE_IN_MBS / DEFAULT_BUFFERING_INTERVAL_IN_SECONDS / the S3BackupMode defaults) and lines 296-331 (the ExtendedS3 arm that applies them).

## Steps to Reproduce
1. Replay fixture firehose-kfi-traj-directput-silently-drops-kinesis-source step task#14: CreateDeliveryStream with an IcebergDestinationConfiguration that supplies only CatalogConfiguration, DestinationTableConfigurationList, RoleARN and S3Configuration, wait for ACTIVE, then DescribeDeliveryStream and read Destinations[0].IcebergDestinationDescription — AppendOnly, BufferingHints, RetryOptions and S3BackupMode must all be present with Firehose's defaults.
2. Replay fixture firehose-kfi-traj-rerun-reference-script step task#13: same, with BufferingHints supplied on the create — AppendOnly, RetryOptions and S3BackupMode must still come back.
