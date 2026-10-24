Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__lambda__s3.firehose-http-endpoint-request-envelope-conforms-to-the-documented-protocol
Issue-Key: firehose__lambda__s3.firehose-http-endpoint-request-envelope-conforms-to-the-documented-protocol|_poll_until#22|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# Firehose HTTP endpoint destination is never invoked: the documented delivery request envelope is never sent

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
One net-new root cause behind the firehose__lambda__s3 failure.

Cause 1 of 1: a stream with an HttpEndpointDestinationConfiguration never POSTs anything to the configured endpoint, so the documented delivery envelope cannot be observed.
AWS: Smithy Firehose models HttpEndpointDestinationConfiguration (EndpointConfiguration Url/Name/AccessKey, RequestConfiguration with ContentEncoding and CommonAttributes, BufferingHints, RetryOptions, S3BackupMode plus an S3Configuration), and PutRecord documents that "Firehose buffers records before delivering them to the destination". AWS then delivers each buffered batch to the endpoint as an HTTP POST whose JSON body carries requestId, timestamp and a records array of base64 data items, with the configured common attributes and content encoding, and parks failed batches in the S3 backup.
Emulator: PutRecord/PutRecordBatch answer 200 with RecordIds and only append the data to an in-process buffer (services/service-com-amazonaws-firehose-firehose-20150804/src/state.rs, DeliveryStream.records, drained only by the 24 h RECORD_RETENTION expiry); the crate declares no delivery component (src/lib.rs: ops, scaffolded, service, state), so the endpoint is never called and the S3 backup stays empty. The check waits for the captured request and runs out the suite budget ("Error: Check timed out after 60.0s").
Why: anyone integrating a custom HTTP sink (or a Lambda function URL acting as one) cannot exercise or validate the request envelope, the common attributes, the content encoding or the retry/backup behavior; against the emulator the integration appears to work while nothing is ever delivered.
Fix: the Firehose destination-delivery (flush) component with an HTTP endpoint writer that renders the documented request envelope (requestId, timestamp, records[].data) from buffered records, applies RequestConfiguration (ContentEncoding, CommonAttributes) and routes failures to the destination's S3 backup.
Traces: firehose__lambda__s3.firehose-http-endpoint-request-envelope-conforms-to-the-documented-protocol.

## Evidence
Check verdict: "Error: Check timed out after 60.0s".
Expected (real AWS): after records are put into a stream whose destination is an HTTP endpoint, the endpoint receives a POST whose body is the documented envelope (requestId, timestamp, records[] of base64-encoded payloads) carrying the configured common attributes.
Actual (emulator): the put calls return 200 with RecordIds, the endpoint is never invoked and the S3 backup receives nothing, so the capture the check reads never appears.

## Steps to Reproduce
1. Boot the emulator; stand up an HTTP sink that records the requests it receives (the check uses a Lambda-backed endpoint) and create an S3 backup bucket plus a firehose-trusted role.
2. firehose.create_delivery_stream with HttpEndpointDestinationConfiguration {EndpointConfiguration {Url, Name}, RequestConfiguration {ContentEncoding, CommonAttributes}, BufferingHints {SizeInMBs:1, IntervalInSeconds:60}, S3BackupMode, S3Configuration}; wait for ACTIVE.
3. put_record_batch with a few payloads.
4. Expect a POST at the endpoint carrying the documented envelope within the buffer window; observe no request is ever made and the S3 backup stays empty.
5. Re-replay the check id in trace_ids.
