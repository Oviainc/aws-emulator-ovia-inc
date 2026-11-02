Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.destination-retry-duration-respected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.destination-retry-duration-respected|run#24|Timeout|time.sleep(2)
Issue-Commit: 447d12c9dbd680364e362fd1ed9112e555aaa4e2

# firehose: the Splunk destination has no delivery path — a record put to a SplunkDestinationConfiguration stream is never attempted against the HEC endpoint and, under S3BackupMode FailedEventsOnly, never lands in the S3 backup with a "Splunk." errorCode (1 cause)

> _Note: this issue was identified against the code base at commit `447d12c9dbd680364e362fd1ed9112e555aaa4e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1: A delivery stream created with `SplunkDestinationConfiguration` accepts records but never delivers or parks them. Nothing is ever attempted against the HEC endpoint, and with `S3BackupMode` = `FailedEventsOnly` no failed-event object is ever written to the configured S3 backup location, so the record remains buffered indefinitely and is unobservable anywhere.

AWS: Smithy (com.amazonaws.firehose) models `SplunkDestinationConfiguration`/`SplunkDestinationDescription` with `HECEndpoint`, `HECEndpointType`, `HECToken`, `HECAcknowledgmentTimeoutInSeconds` (@range 180..600), `RetryOptions` -> `SplunkRetryOptions.DurationInSeconds` (@range 0..7200) and `S3BackupMode` -> enum `SplunkS3BackupMode` with values `FailedEventsOnly` | `AllEvents`. `SplunkDestinationDescription$S3BackupMode` documents: "Defines how documents should be delivered to Amazon S3. When set to FailedDocumentsOnly, Firehose writes any data that could not be indexed to the configured Amazon S3 destination." `SplunkRetryOptions$DurationInSeconds` documents: "The total amount of time that Firehose spends on retries. This duration starts after the initial attempt to send data to Splunk fails." So with `DurationInSeconds` 0 Firehose makes one delivery attempt to the HEC endpoint, gives up on failure, and — because the backup mode is `FailedEventsOnly` — writes the failed event into the S3 destination the `S3Configuration` names. Recorded behavior only for the shape of that backup record: Firehose writes one JSON document per line, the failed event's original bytes carried base64-encoded under `rawData` and the failure classified by an `errorCode` whose fixed prefix is the destination family name `Splunk.` (the suffix is the failure class Firehose observed — e.g. a connection/acknowledgment failure class — and is a value from the attempt, not part of the template). Nothing is written implausibly early: the stream in this check sets no `BufferingHints`, so the Splunk default buffer interval elapses before the first attempt, and the check asserts the bucket is still empty 30 s after the put.

Emulator: `PutRecord` is accepted (HTTP 200, a RecordId is minted) and `DescribeDeliveryStream` reports the stream ACTIVE with the Splunk settings echoed, but no S3 object ever appears in the backup bucket, so the check's bounded poll never finds a backup record and the check is killed by the harness: "Check timed out after 60.0s". In `services/service-com-amazonaws-firehose-firehose-20150804/src/state/delivery.rs`, `s3_targets()` returns an S3 target for `DestinationConfigKind::Splunk(config)` ONLY when `backs_up_all_records(config.s3_backup_mode)` is true (i.e. `AllEvents`, see `ALL_RECORDS_BACKUP_MODES`); under `FailedEventsOnly` it yields no target at all. There is also no Splunk counterpart to `http_endpoint_targets()`/`post_to_endpoint()`/`backup_object()`: the HEC endpoint is never contacted, no attempt is ever recorded as failed, and consequently no error envelope (`rawData` + `errorCode`) is ever produced for a Splunk destination. The records simply sit in the stream buffer until the 24-hour retention reclaims them.

Why: a client that uses Splunk as a destination with `FailedEventsOnly` backup — the default and recommended configuration for not losing undeliverable events — sees its data silently disappear. The producer gets a 200 from PutRecord/PutRecordBatch, and neither the Splunk index nor the S3 backup ever shows the event, so no consumer can distinguish "delivered" from "lost". Any runbook or test that asserts failed events are recoverable from the backup prefix, or that classifies failures by the `Splunk.*` errorCode, cannot be exercised at all, and `RetryOptions.DurationInSeconds` is echoed but has no observable effect.

Fix: the Splunk delivery path in the stream's delivery engine — give `DestinationConfigKind::Splunk` a destination target analogous to the HTTP-endpoint one (attempt the buffer at `HECEndpoint` with `HECToken`/`HECEndpointType`, bounded by `HECAcknowledgmentTimeoutInSeconds` and `RetryOptions.DurationInSeconds`), and on exhaustion write the failed events into the `S3Configuration` location as the per-record error envelope (base64 `rawData` plus a `Splunk.`-prefixed `errorCode` and its message) whenever `S3BackupMode` is `FailedEventsOnly`, in addition to the existing `AllEvents` copy-everything behavior.

Traces: firehose.destination-retry-duration-respected (file/line citations above: services/service-com-amazonaws-firehose-firehose-20150804/src/state/delivery.rs, `s3_targets` Splunk arm ~line 742, `ALL_RECORDS_BACKUP_MODES` ~line 452, `backup_object` ~line 1517).

No open issue covers this: the nearest ones (20261029T175617Z-firehose-6-net-new-root-causes-bufferinghints-are.md, 20261025T083505Z-firehose-s3-delivery-ignores-bufferinghints-the-destination-role.md, 20261028T050836Z-firehose-lambda-s3-an-httpendpointdestination-delivery-stream-produces.md) concern the S3 and HttpEndpoint destination families, not the Splunk destination's delivery and FailedEventsOnly backup.

## Evidence
Check: firehose.destination-retry-duration-respected (verdict: mismatch).

Expected (AWS): CreateDeliveryStream with SplunkDestinationConfiguration{HECEndpoint=<unroutable TEST-NET-3 https URL>, HECEndpointType=Event, HECToken=<guid>, HECAcknowledgmentTimeoutInSeconds=180, RetryOptions={DurationInSeconds:0}, S3BackupMode=FailedEventsOnly, S3Configuration={RoleARN,BucketARN}} -> DescribeDeliveryStream reports those values (this part the emulator already satisfies); a single PutRecord is then NOT in the backup bucket 30 s later, and WITHIN the bounded poll window an S3 object appears in the backup bucket containing one JSON record per line whose base64 `rawData` decodes to the put payload and whose `errorCode` begins with the fixed prefix "Splunk.".

Actual (emulator): PutRecord returns 200, and ListObjectsV2 on the backup bucket returns no Contents for the entire poll window — no delivered object, no error-prefix object, no backup object ever. The check is terminated by the harness with "Error: Check timed out after 60.0s" while still polling for the backup object; the assertion that would have been reached (`error_code.startswith("Splunk.")`) is never gradeable because no backup record exists.

Code evidence: src/state/delivery.rs `s3_targets()` — `DestinationConfigKind::Splunk(config) => { if backs_up_all_records(config.s3_backup_mode.as_ref()) { targets.extend(plain_target(&config.s3_configuration)); } }`, where `ALL_RECORDS_BACKUP_MODES = ["Enabled", "AllDocuments", "AllEvents", "AllData"]`. `FailedEventsOnly` matches none of them, so the Splunk stream has zero delivery targets; `http_endpoint_targets()` early-returns for any non-HttpEndpoint kind, so `backup_object()`/`post_to_endpoint()` never run for Splunk either.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role trusting firehose.amazonaws.com with s3:PutObject etc. on that bucket.
2. CreateDeliveryStream(DeliveryStreamName=<name>, SplunkDestinationConfiguration={HECEndpoint: "https://203.0.113.7:8088/services/collector/event" (RFC 5737 TEST-NET-3, unroutable), HECEndpointType: "Event", HECToken: <any guid>, HECAcknowledgmentTimeoutInSeconds: 180, RetryOptions: {DurationInSeconds: 0}, S3BackupMode: "FailedEventsOnly", S3Configuration: {RoleARN: <role arn>, BucketARN: "arn:aws:s3:::<bucket>"}}).
3. Poll DescribeDeliveryStream until DeliveryStreamStatus == ACTIVE; confirm SplunkDestinationDescription echoes RetryOptions.DurationInSeconds=0, HECAcknowledgmentTimeoutInSeconds=180, S3BackupMode=FailedEventsOnly.
4. PutRecord(DeliveryStreamName=<name>, Record={Data: b"<marker>\n"}).
5. Wait 30 s and ListObjectsV2(Bucket=<bucket>): empty, as AWS also shows.
6. Keep polling ListObjectsV2/GetObject on the bucket. AWS eventually yields a backup object whose lines are JSON records with base64 `rawData` decoding to "<marker>\n" and an `errorCode` starting with "Splunk."; the emulator yields no object at all for the whole window, and the check times out.
