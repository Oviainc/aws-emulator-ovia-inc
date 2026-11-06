Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.get-table-bucket-replication-on-unreplicated-bucket-rejected s3tables.delete-table-bucket-replication-without-config-rejected s3tables.get-table-bucket-metrics-configuration-not-configured-rejected s3tables.get-table-bucket-metrics-configuration-without-put-rejected s3tables.metrics-configuration-disabled-by-default s3tables.put-and-get-table-bucket-metrics-configuration s3tables.put-and-get-table-bucket-metrics-configuration-reflects-settings s3tables.put-table-record-expiration-configuration-out-of-range-rejected s3tables.record-expiration-configuration-rejected-for-standard-table s3tables.record-expiration-days-precision-preserved s3tables.put-table-record-expiration-configuration-rejected-for-customer-table s3tables.table-record-expiration-rejected-for-user-created-table s3tables.get-table-record-expiration-job-status-after-put s3tables.record-expiration-job-status-transitions s3tables.table-record-expiration-job-status-transitions
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.get-table-bucket-replication-on-unreplicated-bucket-rejected|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException on unreplicated bucket, got {code!r}"
Issue-Key: s3tables.delete-table-bucket-replication-without-config-rejected|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: s3tables.get-table-bucket-metrics-configuration-not-configured-rejected|run#4|AssertionError|assert code_before == "NotFoundException", f"expected NotFoundException, got {code_before}"
Issue-Key: s3tables.get-table-bucket-metrics-configuration-without-put-rejected|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: s3tables.metrics-configuration-disabled-by-default|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException by default, got {code}"
Issue-Key: s3tables.put-and-get-table-bucket-metrics-configuration|run#6|AssertionError|assert pre_code == "NotFoundException", f"expected NotFoundException before Put, got {pre_code}"
Issue-Key: s3tables.put-and-get-table-bucket-metrics-configuration-reflects-settings|run#4|AssertionError|assert pre_code == "NotFoundException", f"expected NotFoundException before Put, got {pre_code}"
Issue-Key: s3tables.put-table-record-expiration-configuration-out-of-range-rejected|run#6|AssertionError|assert code == "MethodNotAllowedException", f"expected MethodNotAllowedException, got {code}"
Issue-Key: s3tables.record-expiration-configuration-rejected-for-standard-table|run#6|AssertionError|assert put_code == "MethodNotAllowedException", (
Issue-Key: s3tables.record-expiration-days-precision-preserved|run#10|AssertionError|assert put_code == "MethodNotAllowedException", (
Issue-Key: s3tables.put-table-record-expiration-configuration-rejected-for-customer-table|run#6|AssertionError|assert get_code == "MethodNotAllowedException", f"get_code={get_code}"
Issue-Key: s3tables.table-record-expiration-rejected-for-user-created-table|run#12|AssertionError|assert get_code == "MethodNotAllowedException", f"expected MethodNotAllowedException, got {get_code!r}"
Issue-Key: s3tables.get-table-record-expiration-job-status-after-put|run#6|AssertionError|assert before_code == "MethodNotAllowedException", (
Issue-Key: s3tables.record-expiration-job-status-transitions|run#6|AssertionError|assert code == "MethodNotAllowedException", (
Issue-Key: s3tables.table-record-expiration-job-status-transitions|run#6|AssertionError|assert status_code == "MethodNotAllowedException", (
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# s3tables: six replication / metrics / record-expiration operations are unimplemented scaffold stubs answering 501 NotImplemented

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableBucketMetricsConfiguration

## Description
Six s3tables operations are still `//! SCAFFOLDED, NOT IMPLEMENTED` stubs whose `handle` returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, so every call answers HTTP 501 with error code `NotImplemented` and message "operation <Op> is scaffolded but not implemented in this emulator". Each of the six is a separate operation with its own handler file and its own AWS contract, so each is its own cause; all six must be closed before this issue is submitted. None of these six is covered by the auto-filed scaffold issues of this run (those name PutTableBucketReplication, PutTableReplication, TagResource, PutTableBucketEncryption, GetTableBucketEncryption, PutTableBucketMetricsConfiguration, DeleteTableBucketMetricsConfiguration, GetTableEncryption, PutTableMaintenanceConfiguration, GetTableBucketStorageClass, PutTableBucketStorageClass, GetTableMaintenanceJobStatus) — these six never produced a direct `error` verdict because every check reaches them inside a rejection-probe helper, so they surfaced as code mismatches instead and were never auto-filed.

Cause 1 of 6: GetTableBucketReplication.
AWS: on a table bucket that has no replication configuration, 404 NotFoundException with message "The specified replication configuration does not exist." — fixed text, no variable part (recorded behavior; Smithy models NotFoundException(404) for the operation, and the success shape returns required `configuration` + `versionToken`).
Emulator: 501 `NotImplemented`.
Why: a client that distinguishes "no replication configured" from a server fault cannot: 501 is not a modeled error for the operation, so SDK error matching on NotFoundException never fires and the call looks like an emulator outage.
Fix: the operation's handler (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_replication.rs) plus the replication record in the service state.
Traces: s3tables.get-table-bucket-replication-on-unreplicated-bucket-rejected.

Cause 2 of 6: DeleteTableBucketReplication.
AWS: on a bucket that never had replication configured, 404 NotFoundException with the same fixed message "The specified replication configuration does not exist." (not a silent no-op, and not ConflictException); once a configuration exists the identical call succeeds (recorded behavior).
Emulator: 501 `NotImplemented`.
Why: the delete can neither be performed nor correctly refused, so teardown paths that tolerate the documented 404 break.
Fix: the operation's handler (src/ops/delete_table_bucket_replication.rs) + replication state.
Traces: s3tables.delete-table-bucket-replication-without-config-rejected.

Cause 3 of 6: GetTableBucketMetricsConfiguration.
AWS: metrics are disabled by default, so on a fresh table bucket the read is refused with 404 NotFoundException (fixed text reported by the check as the not-configured class); after a successful PutTableBucketMetricsConfiguration the read returns the stored settings (recorded behavior).
Emulator: 501 `NotImplemented` for both the before-Put read and the read-back.
Why: the default-off state is unobservable and no client can confirm a metrics configuration it just wrote.
Fix: the operation's handler (src/ops/get_table_bucket_metrics_configuration.rs), reading the per-bucket metrics configuration from state.
Traces: s3tables.get-table-bucket-metrics-configuration-not-configured-rejected, s3tables.get-table-bucket-metrics-configuration-without-put-rejected, s3tables.metrics-configuration-disabled-by-default, s3tables.put-and-get-table-bucket-metrics-configuration, s3tables.put-and-get-table-bucket-metrics-configuration-reflects-settings.

Cause 4 of 6: PutTableRecordExpirationConfiguration.
AWS: record expiration is only available for AWS-managed tables; for a customer-created ICEBERG table a well-formed request (any `days` inside the modeled range, e.g. 1 or 30) is refused with 405 MethodNotAllowedException and message exactly "Table record expiration configuration API calls to this table are not allowed" — fixed text, no variable part. Smithy: PutTableRecordExpirationConfiguration models MethodNotAllowedException(405), BadRequestException(400), NotFoundException(404); value.settings.days targets PositiveInteger @range(1..2147483647).
Emulator: 501 `NotImplemented`.
Why: the eligibility gate that is this operation's entire observable behavior on customer tables is missing, so a client cannot tell "not allowed on this table" from "service broken".
Fix: the operation's handler (src/ops/put_table_record_expiration_configuration.rs) — implement the eligibility gate and the stored configuration.
Traces: s3tables.put-table-record-expiration-configuration-out-of-range-rejected, s3tables.record-expiration-configuration-rejected-for-standard-table, s3tables.record-expiration-days-precision-preserved.

Cause 5 of 6: GetTableRecordExpirationConfiguration.
AWS: for a customer-created table, 405 MethodNotAllowedException with the same fixed message "Table record expiration configuration API calls to this table are not allowed" (recorded behavior).
Emulator: 501 `NotImplemented`.
Why: as cause 4, on the read path — the "effect absent" read every client uses to confirm nothing was persisted cannot be performed.
Fix: the operation's handler (src/ops/get_table_record_expiration_configuration.rs).
Traces: s3tables.put-table-record-expiration-configuration-rejected-for-customer-table, s3tables.table-record-expiration-rejected-for-user-created-table.

Cause 6 of 6: GetTableRecordExpirationJobStatus.
AWS: for a customer-created / freshly created table, 405 MethodNotAllowedException with the same fixed message "Table record expiration configuration API calls to this table are not allowed" (recorded behavior).
Emulator: 501 `NotImplemented`.
Why: as cause 4, on the job-status path.
Fix: the operation's handler (src/ops/get_table_record_expiration_job_status.rs).
Traces: s3tables.get-table-record-expiration-job-status-after-put, s3tables.record-expiration-job-status-transitions, s3tables.table-record-expiration-job-status-transitions.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout (s3tables 1/110 passed). Each of the 15 checks below reaches one of the six operations and receives error code `NotImplemented` (HTTP 501) with message "operation <Op> is scaffolded but not implemented in this emulator":
- GetTableBucketReplication: s3tables.get-table-bucket-replication-on-unreplicated-bucket-rejected — "expected NotFoundException on unreplicated bucket, got 'NotImplemented'".
- DeleteTableBucketReplication: s3tables.delete-table-bucket-replication-without-config-rejected — "expected NotFoundException, got 'NotImplemented'".
- GetTableBucketMetricsConfiguration: s3tables.get-table-bucket-metrics-configuration-not-configured-rejected and ...-without-put-rejected — "expected NotFoundException, got NotImplemented"; s3tables.metrics-configuration-disabled-by-default — "expected NotFoundException by default, got NotImplemented"; s3tables.put-and-get-table-bucket-metrics-configuration and ...-reflects-settings — "expected NotFoundException before Put, got NotImplemented".
- PutTableRecordExpirationConfiguration: s3tables.put-table-record-expiration-configuration-out-of-range-rejected — "expected MethodNotAllowedException, got NotImplemented"; s3tables.record-expiration-configuration-rejected-for-standard-table — "PutTableRecordExpirationConfiguration on a user-created ICEBERG table returned 'NotImplemented', expected 'MethodNotAllowedException'"; s3tables.record-expiration-days-precision-preserved — "days=1: expected MethodNotAllowedException, got 'NotImplemented'".
- GetTableRecordExpirationConfiguration: s3tables.put-table-record-expiration-configuration-rejected-for-customer-table — "get_code=NotImplemented"; s3tables.table-record-expiration-rejected-for-user-created-table — "expected MethodNotAllowedException, got 'NotImplemented'".
- GetTableRecordExpirationJobStatus: s3tables.get-table-record-expiration-job-status-after-put — "GetTableRecordExpirationJobStatus on a freshly created customer table returned 'NotImplemented', expected 'MethodNotAllowedException'"; s3tables.record-expiration-job-status-transitions and s3tables.table-record-expiration-job-status-transitions — same code, expected MethodNotAllowedException.
Source confirmation (all six carry the scaffold marker on line 4 and return `Scaffolded::not_implemented`): services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_bucket_replication.rs:175, delete_table_bucket_replication.rs:4, get_table_bucket_metrics_configuration.rs:4, put_table_record_expiration_configuration.rs:193, get_table_record_expiration_configuration.rs:4, get_table_record_expiration_job_status.rs:4.
Model check (smithy_query on services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json): PutTableRecordExpirationConfiguration models MethodNotAllowedException(405)/BadRequestException(400)/NotFoundException(404); GetTableBucketReplication models NotFoundException(404). No operation in this service models anything answering 501.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-aws-v1.
2. Create a table bucket, then call GetTableBucketReplication and DeleteTableBucketReplication on it — expect 404 NotFoundException "The specified replication configuration does not exist.", observe 501 NotImplemented.
3. On the same fresh bucket call GetTableBucketMetricsConfiguration — expect 404 NotFoundException (metrics off by default), observe 501 NotImplemented.
4. Create a namespace and an ICEBERG table under the bucket, then call PutTableRecordExpirationConfiguration (value={"status":"enabled","settings":{"days":30}}), GetTableRecordExpirationConfiguration and GetTableRecordExpirationJobStatus on that table ARN — expect 405 MethodNotAllowedException "Table record expiration configuration API calls to this table are not allowed" from all three, observe 501 NotImplemented.
