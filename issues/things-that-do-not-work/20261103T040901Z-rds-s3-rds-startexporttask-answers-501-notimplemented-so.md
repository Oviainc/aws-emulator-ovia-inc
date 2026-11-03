Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow rds__s3.cli-rds-export-invalid-bucket-name-negative rds__s3.rds-export-task-kms-key-missing-permission-negative
Issue-Key: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow|_run_aws_retrying#14|AssertionError|raise AssertionError(
Issue-Key: rds__s3.cli-rds-export-invalid-bucket-name-negative|_start_export_task_cli#12|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-kms-key-missing-permission-negative|run#11|AssertionError|assert error["Code"] == "KMSKeyNotAccessibleFault", (
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# rds__s3: RDS StartExportTask answers 501 NotImplemented, so no snapshot export to S3 can be started and its parameter refusals never happen

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
One root cause, grounded in three failing checks under `### rds__s3`.

Cause 1 of 1: RDS StartExportTask is an unimplemented scaffold, so the whole snapshot-export-to-S3 path — including its input validation — is unreachable.
AWS: recorded behavior (the operation's own refusals are what the checks assert). A valid StartExportTask against an available DB snapshot is ACCEPTED and returns an export task whose `Status` progresses (STARTING → … → COMPLETE) and whose objects land under `S3Bucket`/`S3Prefix`. Its parameter refusals are modeled faults, not NotImplemented: an `S3BucketName` that is not a legal bucket name is refused with an invalid-parameter fault (HTTP 400), and an `KmsKeyId` the export role cannot use is refused `<Code>KMSKeyNotAccessibleFault</Code>` — the message's fixed text is AWS's wording for that fault and its variable part is the key id/ARN taken from the request.
Emulator: every call answers HTTP 501 `<Code>NotImplemented</Code>` with "operation StartExportTask is scaffolded but not implemented in this emulator". The KMS-permission check therefore reports "expected KMSKeyNotAccessibleFault, got NotImplemented"; the CLI workflow fails `rc=254` with that same NotImplemented body; and the invalid-bucket-name check, which retries while waiting for IAM propagation, never gets a verdict and ends `Error: Check timed out after 60.0s`.
Why: a client cannot export a snapshot to S3 at all, and — because 501 is returned for every input — it cannot distinguish "the service is not available" from "my bucket name or KMS key is wrong", so the refusal-driven remediation path does not exist. The open issue 20261029T072224Z-rds-s3-startexporttask-is-an-unimplemented-scaffold-stub.md is adjacent and owns a different check; these three replay keys are not carried by it.
Fix: the StartExportTask handler (services/service-com-amazonaws-rds-amazonrdsv19, the StartExportTask op) — the export-task state machine plus its S3BucketName / S3Prefix / KmsKeyId / IamRoleArn validation and the DescribeExportTasks view it feeds.
Traces: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow, rds__s3.cli-rds-export-invalid-bucket-name-negative, rds__s3.rds-export-task-kms-key-missing-permission-negative.

## Evidence
rds__s3.cli-rds-create-snapshot-export-to-s3-workflow: "AssertionError: start-export-task (waiting on IAM role/policy propagation) failed (rc=254): aws: [ERROR]: An error occurred (NotImplemented) when calling the StartExportTask operation: operation StartExportTask is scaffolded but not implemented in this emulator".
rds__s3.rds-export-task-kms-key-missing-permission-negative: "AssertionError: expected KMSKeyNotAccessibleFault, got NotImplemented".
rds__s3.cli-rds-export-invalid-bucket-name-negative: "Error: Check timed out after 60.0s" — the check retries the start-export-task call waiting for a parameter refusal that never arrives.
Expected: a 200 export-task document for the valid call, and modeled 400-class faults (invalid parameter / KMSKeyNotAccessibleFault) for the two negative cases.

## Steps to Reproduce
1. Create a DB instance and a manual snapshot, create an export role trusted by export.rds.amazonaws.com and a destination bucket.
2. Call rds:StartExportTask with a valid ExportTaskIdentifier, SourceArn, S3BucketName, IamRoleArn and KmsKeyId — expect 200 and an export task that progresses to COMPLETE with objects in the bucket; observe 501 NotImplemented.
3. Repeat with an illegal S3BucketName — expect a 400 invalid-parameter fault; observe 501.
4. Repeat with a KMS key the role cannot use — expect KMSKeyNotAccessibleFault; observe 501.
5. Re-replay the three check ids in trace_ids.
