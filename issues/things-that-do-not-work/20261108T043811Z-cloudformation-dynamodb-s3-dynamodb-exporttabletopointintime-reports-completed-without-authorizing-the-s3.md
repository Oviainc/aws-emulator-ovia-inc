Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.dynamodb-export-to-s3-missing-bucket-permission-negative cloudformation__dynamodb__s3.s3-bucket-policy-denies-dynamodb-export-service-principal-negative
Issue-Key: cloudformation__dynamodb__s3.dynamodb-export-to-s3-missing-bucket-permission-negative|run#13|AssertionError|assert final["ExportStatus"] == "FAILED", (
Issue-Key: cloudformation__dynamodb__s3.s3-bucket-policy-denies-dynamodb-export-service-principal-negative|run#16|AssertionError|assert final["ExportStatus"] == "FAILED", (
Issue-Commit: 8f7c6ce558c7d939146a7c42cc95df5134e5d406

# DynamoDB ExportTableToPointInTime reports COMPLETED without authorizing the S3 write (no identity grant, and over an explicit bucket-policy Deny)

> _Note: this issue was identified against the code base at commit `8f7c6ce558c7d939146a7c42cc95df5134e5d406`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
One cause, exercised by two checks.

Cause 1 of 1: the export path never evaluates S3 authorization for the exporting principal, so an export whose caller cannot legally write the destination still reaches ExportStatus COMPLETED. Two independent authorization inputs are both ignored: the caller's identity policy (no s3:* at all) and the destination bucket's resource policy (an explicit Deny on the caller's role ARN).
Failing check ids: cloudformation__dynamodb__s3.dynamodb-export-to-s3-missing-bucket-permission-negative, cloudformation__dynamodb__s3.s3-bucket-policy-denies-dynamodb-export-service-principal-negative

AWS: recorded behavior only for the terminal state (ExportTableToPointInTime is accepted asynchronously; the authorization verdict surfaces on the export description, not as a synchronous error). Recorded: ExportTableToPointInTime is accepted (HTTP 200, an ExportDescription with ExportStatus IN_PROGRESS) and then the export terminates with ExportStatus = FAILED and a FailureCode naming the S3 access failure (S3AccessDenied), with no objects written under the destination prefix, whenever the write the export must perform is not authorized — both when the calling role holds dynamodb:ExportTableToPointInTime but zero s3 permissions and the bucket grants nothing back, and when the role's identity policy allows s3:PutObject/s3:AbortMultipartUpload/s3:PutObjectAcl/s3:ListBucket on the bucket but the bucket policy carries an explicit Deny for that role's ARN (an explicit Deny outranks any Allow). The same role exporting to an unrestricted bucket reaches COMPLETED with objects written — both checks carry that positive control, so the divergence is not "everything completes" but "the denial is not evaluated". The FailureCode/FailureMessage values name the bucket and the principal from the request and from IAM state, never a fixed literal.

Emulator: both exports reach ExportStatus COMPLETED. The replay's assertion fires on the terminal state: "export with no S3 permission anywhere should FAIL, got COMPLETED" and "export into an explicitly-denied bucket should FAIL, got COMPLETED".

Why: this is an overly-permissive divergence, the most dangerous kind. A client validating least-privilege for a backup/export pipeline — the exact purpose of both checks' GROUP "overly-permissive" — gets a green COMPLETED from an export real AWS refuses, so a policy regression (a missing s3:PutObject grant, or a deliberate Deny guarding a bucket) is proven safe by the emulator and only discovered against real AWS. Negative-path tests that assert FAILED/S3AccessDenied cannot pass here, and code that reads ExportStatus to decide whether the data landed will treat a non-export as a successful one.

Fix: the ExportTableToPointInTime handler's export execution/state machine — before and while writing the export artifacts it must authorize the write as the calling principal against both the identity policy and the destination bucket policy, and on refusal terminate the export with ExportStatus FAILED plus the S3AccessDenied failure code/message instead of COMPLETED, leaving the destination prefix empty.

Traces: cloudformation__dynamodb__s3.dynamodb-export-to-s3-missing-bucket-permission-negative (role with dynamodb:ExportTableToPointInTime and no S3 permission, bucket with no policy); cloudformation__dynamodb__s3.s3-bucket-policy-denies-dynamodb-export-service-principal-negative (role allowed by identity policy on both buckets, destination bucket policy explicitly Denies that role ARN).

## Evidence
Check: cloudformation__dynamodb__s3.dynamodb-export-to-s3-missing-bucket-permission-negative
  Setup: table with PITR enabled and one item; bucket created with no bucket policy; role trusted for the export with extra_statements=[] (no s3 permission whatsoever); export issued as that role via ExportTableToPointInTime(TableArn=..., S3Bucket=<noperm-bucket>, ExportFormat="DYNAMODB_JSON").
  Expected (AWS): the export terminates with ExportStatus == "FAILED" and no objects in the bucket.
  Actual (emulator): AssertionError: export with no S3 permission anywhere should FAIL, got COMPLETED

Check: cloudformation__dynamodb__s3.s3-bucket-policy-denies-dynamodb-export-service-principal-negative
  Setup: role whose identity policy Allows s3:PutObject, s3:AbortMultipartUpload, s3:PutObjectAcl, s3:ListBucket on arn:aws:s3:::<denied-bucket>[/*] and arn:aws:s3:::<ok-bucket>[/*]; <denied-bucket> carries an explicit Deny for that role's ARN; <ok-bucket> carries no Deny (positive control).
  Expected (AWS): the export into <denied-bucket> terminates FAILED, the failure names an explicit deny (S3AccessDenied), and no objects land; the same role's export into <ok-bucket> reaches COMPLETED.
  Actual (emulator): AssertionError: export into an explicitly-denied bucket should FAIL, got COMPLETED

## Steps to Reproduce
A — identity policy absent:
1. CreateTable, enable PITR, PutItem one item.
2. CreateBucket <noperm>; attach no bucket policy.
3. Create an IAM role trusted for the export with only dynamodb:ExportTableToPointInTime (no s3 actions); AssumeRole it.
4. As that role: ExportTableToPointInTime(TableArn=<table>, S3Bucket=<noperm>, ExportFormat=DYNAMODB_JSON).
5. Poll DescribeExport until terminal — the emulator reports COMPLETED; AWS reports FAILED with an S3 access failure and leaves <noperm> empty.

B — explicit bucket-policy Deny:
1. Same table/PITR/item. CreateBucket <denied> and <ok>.
2. Create a role whose identity policy Allows s3:PutObject/s3:AbortMultipartUpload/s3:PutObjectAcl/s3:ListBucket on both buckets plus the export action.
3. PutBucketPolicy on <denied> with an explicit Deny whose Principal is that role's ARN.
4. As that role: ExportTableToPointInTime into <denied>, poll to terminal — the emulator reports COMPLETED; AWS reports FAILED (explicit deny) with nothing written.
5. Repeat into <ok> — both AWS and the emulator reach COMPLETED, confirming the wiring is otherwise healthy.
