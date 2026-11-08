Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.get-table-bucket-metrics-configuration-not-configured-rejected s3tables.get-table-bucket-metrics-configuration-without-put-rejected s3tables.metrics-configuration-disabled-by-default s3tables.put-and-get-table-bucket-metrics-configuration s3tables.put-and-get-table-bucket-metrics-configuration-reflects-settings
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.get-table-bucket-metrics-configuration-not-configured-rejected|run#5|AssertionError|assert message_before == "No such metric configuration exists", (
Issue-Key: s3tables.get-table-bucket-metrics-configuration-without-put-rejected|run#5|AssertionError|assert message == "No such metric configuration exists", (
Issue-Key: s3tables.metrics-configuration-disabled-by-default|run#7|AssertionError|assert message == "No such metric configuration exists", (
Issue-Key: s3tables.put-and-get-table-bucket-metrics-configuration|run#7|AssertionError|assert pre_message == "No such metric configuration exists", (
Issue-Key: s3tables.put-and-get-table-bucket-metrics-configuration-reflects-settings|run#5|AssertionError|assert pre_message == "No such metric configuration exists", (
Issue-Commit: 7bce35011e06a27791111295606361de5e667ce7

# s3tables GetTableBucketMetricsConfiguration returns the wrong NotFoundException message for a bucket with no metrics configuration

> _Note: this issue was identified against the code base at commit `7bce35011e06a27791111295606361de5e667ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableBucketMetricsConfiguration

## Description
One root cause, grounded in 5 failing behavioral checks.

Cause 1 of 1: the NotFoundException message GetTableBucketMetricsConfiguration emits when the named table bucket exists but carries no metrics configuration is emulator-authored text instead of AWS's message.

AWS: Smithy (services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json) models GetTableBucketMetricsConfiguration GET /buckets/{tableBucketARN}/metrics with errors BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500), NotFoundException(404 — "The request was rejected because the specified resource could not be found"), TooManyRequestsException(429). The model does not fix the message string, so the message is recorded behavior only: on a table bucket that has never had PutTableBucketMetricsConfiguration applied, and again after DeleteTableBucketMetricsConfiguration, AWS refuses the read with HTTP 404, error code `NotFoundException` and the message `No such metric configuration exists`. That message is entirely fixed text — it interpolates nothing from the request or from state (no bucket name, no ARN, no id), which is why the checks compare it by exact equality.

Emulator: the status (404) and the error code (`NotFoundException`) are right, but the message is `The specified metrics configuration does not exist.` — different wording and a trailing period. It comes from the constant `crate::state::METRICS_CONFIGURATION_NOT_FOUND_MESSAGE`, used at services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_metrics_configuration.rs:175-179 for the "bucket exists, configuration absent" branch. The divergence reproduces identically in all three positions the checks probe: before any put, after a delete, and after a delete-then-reput cycle's intermediate read.

Why: a client that distinguishes the "metrics were never enabled" condition from other 404s on this path — s3tables has only one NotFoundException shape, so the message is the only discriminator between "no such table bucket" and "no such metric configuration" — mis-classifies the refusal. SDK/tooling and user code that match on the AWS message (or log/assert on it, as these five checks do) see an unknown string and either fall through to a generic error path or treat a normal "metrics off" state as an unexpected failure. The two 404s on this operation are no longer distinguishable the way they are on AWS.

Fix: the error construction in the GetTableBucketMetricsConfiguration handler's absent-configuration branch and the shared message constant it reads — the message for this condition must be AWS's fixed text `No such metric configuration exists` (no trailing period), kept distinct from the bucket-not-found message used by the preceding lookup. Note the open issue 20261102T065133Z-s3tables-encryption-metrics-replication-put-operations-answer-501.md covers the *Put* side answering 501 and 20261101T233620Z-s3tables-deletetablebucketmetricsconfiguration-stub.md covers the Delete stub; neither covers this Get message, which is why it is filed separately.

Traces: s3tables.get-table-bucket-metrics-configuration-not-configured-rejected, s3tables.get-table-bucket-metrics-configuration-without-put-rejected, s3tables.metrics-configuration-disabled-by-default, s3tables.put-and-get-table-bucket-metrics-configuration, s3tables.put-and-get-table-bucket-metrics-configuration-reflects-settings.

## Evidence
Expected (real AWS), GetTableBucketMetricsConfiguration on a table bucket with no metrics configuration:
  HTTP 404
  Error.Code    = "NotFoundException"
  Error.Message = "No such metric configuration exists"   <- wholly fixed text, no request/state values

Actual (emulator), same call:
  HTTP 404
  Error.Code    = "NotFoundException"        (correct)
  Error.Message = "The specified metrics configuration does not exist."   (diverges)

Replay output, identical in all five checks:
  AssertionError: expected the exact not-found message for the missing metrics configuration, got 'The specified metrics configuration does not exist.'

The assertion that fails is `assert message == "No such metric configuration exists"`, immediately after `assert code == "NotFoundException"` passes — so only the message text diverges. Source of the string: services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_metrics_configuration.rs:175-179 -> crate::state::METRICS_CONFIGURATION_NOT_FOUND_MESSAGE.

## Steps to Reproduce
1. s3tables.create_table_bucket(name=<a fresh valid lower-case bucket name>) -> note the returned `arn`.
2. s3tables.get_table_bucket_metrics_configuration(tableBucketARN=<arn>).
   Observe HTTP 404 / NotFoundException with message 'The specified metrics configuration does not exist.'; AWS answers 404 / NotFoundException with message 'No such metric configuration exists'.
3. Also reachable after the configuration has existed and been removed:
   s3tables.put_table_bucket_metrics_configuration(tableBucketARN=<arn>);
   s3tables.delete_table_bucket_metrics_configuration(tableBucketARN=<arn>);
   s3tables.get_table_bucket_metrics_configuration(tableBucketARN=<arn>) -> same diverging message.
4. Re-replay: s3tables.get-table-bucket-metrics-configuration-not-configured-rejected, s3tables.get-table-bucket-metrics-configuration-without-put-rejected, s3tables.metrics-configuration-disabled-by-default, s3tables.put-and-get-table-bucket-metrics-configuration, s3tables.put-and-get-table-bucket-metrics-configuration-reflects-settings.
