Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3__sts.create-export-task-bucket-missing-permission-rejected
Issue-Key: logs__s3__sts.create-export-task-bucket-missing-permission-rejected|_error#10|AssertionError|raise AssertionError(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CreateExportTask is accepted although the destination bucket does not grant the service permission

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
Cause 1 of 1: the destination-permission check is missing, so an export that cannot write is accepted.
AWS: CreateExportTask validates that the destination bucket's policy lets CloudWatch Logs write the export; without that grant the call is refused (InvalidParameterException, HTTP 400) and no task id is returned (Smithy logs CreateExportTask errors: InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException, ResourceAlreadyExistsException).
Emulator: the call returns HTTP 200 with a taskId — 'expected the call to be rejected, but it SUCCEEDED and returned {'taskId': '<id>', ...}' — so the client is handed a task that can never deliver.
Why: a client learns of the misconfiguration only by waiting for an export that silently produces nothing, instead of an immediate validation error.
Fix: destination-bucket permission validation in the CreateExportTask handler of the CloudWatch Logs crate.
Traces: logs__s3__sts.create-export-task-bucket-missing-permission-rejected.

## Evidence
Expected: 400 InvalidParameterException. Actual: 'AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'taskId': '00000000-0000-4000-8000-000000000001', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}'.

## Steps to Reproduce
Replay logs__s3__sts.create-export-task-bucket-missing-permission-rejected: create a bucket whose policy grants CloudWatch Logs nothing and call CreateExportTask against it.
