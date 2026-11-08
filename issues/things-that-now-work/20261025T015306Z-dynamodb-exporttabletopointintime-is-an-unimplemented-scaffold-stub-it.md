Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.dynamodb-export-requires-pitr-negative
Issue-Key: cloudformation__dynamodb__s3.dynamodb-export-requires-pitr-negative|run#9|AssertionError|assert code == "PointInTimeRecoveryUnavailableException", f"unexpected code: {code}"
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# DynamoDB ExportTableToPointInTime is an unimplemented scaffold stub: it answers NotImplemented instead of PointInTimeRecoveryUnavailableException when PITR is off (1 cause)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
Cause 1 of 1: dynamodb ExportTableToPointInTime answers NotImplemented for every request, including the case AWS refuses with a specific modeled error.

AWS: ExportTableToPointInTime(TableArn, S3Bucket, ExportFormat) starts an export to S3 and returns an ExportDescription. When point-in-time recovery was never enabled on the table it is refused with PointInTimeRecoveryUnavailableException (client error, HTTP 400) — the refusal that tells a caller to enable PITR first.
Emulator: every call is refused with 'NotImplemented' — 'operation ExportTableToPointInTime is scaffolded but not implemented in this emulator'.
Why: the error code is not one the operation models, so SDK/retry logic that branches on PointInTimeRecoveryUnavailableException never sees it, and the whole export path (including the positive case that polls until PITR is available) is unusable.
Fix: implement the ExportTableToPointInTime handler in the dynamodb service crate (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/export_table_to_point_in_time.rs) over the table's continuous-backups state, raising PointInTimeRecoveryUnavailableException when PITR is disabled. (The sibling UpdateContinuousBackups stub that the positive path needs is already filed separately.)
Traces: cloudformation__dynamodb__s3.dynamodb-export-requires-pitr-negative.

## Evidence
Replayed suite, check cloudformation__dynamodb__s3.dynamodb-export-requires-pitr-negative:
"assert code == 'PointInTimeRecoveryUnavailableException', f'unexpected code: {code}'
AssertionError: unexpected code: NotImplemented"
The failing call is ddb.export_table_to_point_in_time(TableArn=table_arn, S3Bucket=bucket, ExportFormat='DYNAMODB_JSON') against a table whose PITR was never enabled.

## Steps to Reproduce
1. Boot the emulator and replay the suite; re-run cloudformation__dynamodb__s3.dynamodb-export-requires-pitr-negative.
2. Manual repro: CreateTable, create an S3 bucket, then ExportTableToPointInTime(TableArn=<arn>, S3Bucket=<bucket>, ExportFormat='DYNAMODB_JSON') without enabling PITR — AWS answers 400 PointInTimeRecoveryUnavailableException, the emulator answers NotImplemented.
