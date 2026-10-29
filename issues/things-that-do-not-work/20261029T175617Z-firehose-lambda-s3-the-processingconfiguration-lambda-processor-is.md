Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__lambda__s3.firehose-lambda-transform-rewrites-the-delivered-bytes firehose__lambda__s3.firehose-backup-mode-writes-the-source-record-alongside-the-transformed-one firehose__lambda__s3.firehose-processing-failed-records-land-under-the-error-output-prefix firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message
Issue-Key: firehose__lambda__s3.firehose-lambda-transform-rewrites-the-delivered-bytes|run#16|AssertionError|assert delivered.rstrip(b"\n") == expected, (
Issue-Key: firehose__lambda__s3.firehose-backup-mode-writes-the-source-record-alongside-the-transformed-one|run#14|AssertionError|assert main_body.rstrip(b"\n") == payload[::-1], (
Issue-Key: firehose__lambda__s3.firehose-processing-failed-records-land-under-the-error-output-prefix|run#15|AssertionError|assert ok_body.rstrip(b"\n") == good_marker.encode("utf-8"), (
Issue-Key: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message|_poll_until#19|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# firehose__lambda__s3: the ProcessingConfiguration Lambda processor is never invoked — records are delivered verbatim and the Ok/Dropped/ProcessingFailed verdicts, S3BackupMode and ErrorOutputPrefix are all ignored (2 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Two net-new root causes. The third failure in this service (firehose-http-endpoint-request-envelope-conforms-to-the-documented-protocol) is OWNED by 20261028T050836Z-firehose-lambda-s3-an-httpendpointdestination-delivery-stream-produces.md and is excluded.

=== Cause 1 of 2: a stream whose ProcessingConfiguration declares a Lambda processor never invokes the function; the untransformed source bytes are delivered to the main prefix and no record is ever diverted or backed up ===
AWS: Smithy: ProcessingConfiguration {Enabled, Processors}; a Lambda processor carries the function ARN in its Parameters. Recorded behavior: when Enabled is true, Firehose invokes the function with a `records` array (each entry recordId + base64 `data`) and honours the per-record `result` in the response: `Ok` records are delivered to the destination with the function's returned `data`; `Dropped` records are delivered nowhere; `ProcessingFailed` records are written under ExtendedS3DestinationConfiguration.ErrorOutputPrefix and NOT to the main prefix. With S3BackupMode=Enabled, the UNTRANSFORMED source record is additionally written under S3BackupConfiguration's prefix while the main prefix receives only the transformed bytes.
Emulator: the function is never invoked. Every record is written to the main prefix exactly as it was put: the transform check receives the raw payload instead of the transformed form; the backup-mode check finds the raw source record in the MAIN prefix (where only the transformed record belongs); and the failed-record check finds BOTH the good and the ProcessingFailed-marked record concatenated under the success prefix instead of the bad one being diverted to the error prefix.
Why: every Firehose transformation use case silently degrades to pass-through. A client's enrichment/reformatting never happens, data it asked to drop is delivered, records it expects quarantined under the error prefix are mixed into the good output, and the backup prefix (the only copy of the pre-transform bytes) is not written — so there is no way to reconcile or reprocess.
Fix: the Firehose delivery pipeline's processing stage — invoke the Lambda processor named in ProcessingConfiguration.Processors before buffering, apply the per-record Ok/Dropped/ProcessingFailed verdict, route ProcessingFailed records to ErrorOutputPrefix, and write the source record to the S3 backup prefix when S3BackupMode is Enabled.
Traces: firehose__lambda__s3.firehose-lambda-transform-rewrites-the-delivered-bytes, firehose__lambda__s3.firehose-backup-mode-writes-the-source-record-alongside-the-transformed-one, firehose__lambda__s3.firehose-processing-failed-records-land-under-the-error-output-prefix.

=== Cause 2 of 2: an HttpEndpointDestination stream issues no request, so a rejected batch is never retried and never persisted to S3 with the endpoint's error message ===
AWS: recorded behavior only (HttpEndpointDestinationConfiguration, its RetryOptions and S3BackupMode are modelled; the delivery protocol is not). AWS behavior: Firehose POSTs the documented request envelope to the configured endpoint; when the endpoint answers a rejection, Firehose retries within RetryOptions.DurationInSeconds and then writes the failed records to the S3 backup bucket, each carrying the error message the endpoint returned.
Emulator: no request reaches the endpoint, so neither the retry nor the backup object is ever produced and the check exhausts its budget (`Check timed out after 60.0s`).
Why: a client using an HTTP endpoint destination loses every record with no trace and cannot observe or tune retry behavior. Open issue 20261028T050836Z covers the request-envelope sibling; this check adds the rejected-batch retry-and-persist arm and is not carried by it.
Fix: the Firehose HttpEndpointDestination delivery worker and its S3 backup path (issue the POST, honour RetryOptions, persist failed records with the endpoint's error message).
Traces: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message.

## Evidence
Cause 1 — firehose__lambda__s3.firehose-lambda-transform-rewrites-the-delivered-bytes:
  assert delivered.rstrip(b"\n") == expected
  AssertionError: delivered b'xfm-<id>\n', expected the transformed form b'XFM-<ID>|XFORMED|fn=<function-name>'
  (expected = the function's output: upper-cased payload + '|XFORMED|fn=' + context.function_name)

firehose__lambda__s3.firehose-backup-mode-writes-the-source-record-alongside-the-transformed-one:
  assert main_body.rstrip(b"\n") == payload[::-1]
  AssertionError: main/ held b'bkp-<id>-ZZZ', expected the reversed payload b'ZZZ-<id>-pkb'
  i.e. the main prefix holds the SOURCE bytes; only the backup prefix should.

firehose__lambda__s3.firehose-processing-failed-records-land-under-the-error-output-prefix:
  assert ok_body.rstrip(b"\n") == good_marker.encode("utf-8")
  AssertionError: the success prefix held b'good-<id>\nbad-<id>\n', expected exactly the good record b'good-<id>\n'
  i.e. the record the processor marked ProcessingFailed was delivered to the success prefix instead of ErrorOutputPrefix.

Cause 2 — firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message: Error: Check timed out after 60.0s; no request at the endpoint and no backup object carrying the endpoint's error message.

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Cause 1a (transform): create a Lambda whose handler returns, per record, {recordId, result: 'Ok', data: base64(<rewritten payload>)}; CreateDeliveryStream (DirectPut, ExtendedS3DestinationConfiguration) with ProcessingConfiguration {Enabled: true, Processors: [{Type: Lambda, Parameters: [{LambdaArn: <fn arn>}]}]}; wait ACTIVE; PutRecord a payload; read the delivered object — it holds the original bytes, and the function was never invoked.
3. Cause 1b (backup): add S3BackupMode=Enabled with an S3BackupConfiguration prefix and a transform that rewrites the payload; PutRecord; read both prefixes — the main prefix holds the SOURCE record.
4. Cause 1c (error prefix): have the function return result 'ProcessingFailed' for one record of a two-record batch and set ErrorOutputPrefix; PutRecordBatch; read the success prefix — it contains both records.
5. Cause 2: CreateDeliveryStream with HttpEndpointDestinationConfiguration pointing at an endpoint that rejects the batch, RetryOptions set and S3BackupMode configured; PutRecord; observe no request at the endpoint and no backup object with the endpoint's error message.
