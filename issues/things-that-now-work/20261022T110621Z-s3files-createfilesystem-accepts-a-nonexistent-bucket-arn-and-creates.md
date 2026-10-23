Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3files.create-file-system-unknown-bucket-not-found
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: s3files.create-file-system-unknown-bucket-not-found|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 92ad2730926ec3062b662e938e8ed9f3fac68577

# CreateFileSystem accepts a nonexistent bucket ARN and creates the file system instead of returning 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `92ad2730926ec3062b662e938e8ed9f3fac68577`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem

## Description
Cause 1 of 1: CreateFileSystem does not validate that the S3 bucket named by the `bucket` ARN exists, so a well-formed ARN for a bucket that does not exist is accepted and a file system resource is created.

AWS: CreateFileSystem (Smithy: s3files CreateFileSystem, http PUT /file-systems, success code 201; input member `bucket` target BucketArn, required, pattern ^(arn:aws[a-zA-Z0-9-]*:s3:::.+)$) models ResourceNotFoundException with httpError 404 among its errors (ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400, ThrottlingException 429). The recorded behavior is that a syntactically valid `bucket` ARN naming a bucket that does not exist is REJECTED with 404 ResourceNotFoundException and no file system is created, so the rejected bucket never appears in ListFileSystems; once that same bucket actually exists, the identical call succeeds. Recorded message, as an example only: "The bucket you specified does not exist."

Emulator: the call SUCCEEDS with HTTP 201 and returns a full CreateFileSystemResponse (fileSystemId fs-..., fileSystemArn arn:aws:s3files:<region>:<account>:file-system/fs-..., bucket echoed back, status 'creating', prefix '', roleArn, ownerId). No existence check on `bucket` is performed; the emulator only appears to validate the ARN's shape.

Why: a client that provisions a file system against a mistyped or not-yet-created bucket gets a success and a live-looking fileSystemId that references nothing. Against AWS the client would get a 404 ResourceNotFoundException at request time and could correct the bucket; against the emulator it proceeds, then every downstream operation on that file system (mount targets, access points, synchronization configuration) is exercised against a phantom bucket. Error-handling paths for the not-found class can never be tested, and the state store accumulates file systems AWS would never have created.

Fix: CreateFileSystem's handler/validation in the s3files service — resolve the `bucket` ARN against the S3 bucket state before admitting the file system, and on a miss return the modeled ResourceNotFoundException (HTTP 404) without mutating state, so the bucket also does not appear in ListFileSystems(bucket=...).

Traces: failing check s3files.create-file-system-unknown-bucket-not-found (verdict: mismatch).

## Evidence
Check s3files.create-file-system-unknown-bucket-not-found

Expected (AWS / recorded): create_file_system(bucket='arn:aws:s3:::<nonexistent-bucket>', roleArn=<valid role arn>) is rejected -> Error.Code == 'ResourceNotFoundException', HTTP 404 (Smithy: CreateFileSystem errors include ResourceNotFoundException httpError 404). Recorded message example: 'The bucket you specified does not exist.' ListFileSystems(bucket=<that arn>) then returns an empty fileSystems list.

Actual (emulator): HTTP 201 success —
{'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'creationTime': ..., 'fileSystemArn': 'arn:aws:s3files:us-west-2:000000000000:file-system/fs-0c2c02e40002238a9', 'fileSystemId': 'fs-0c2c02e40002238a9', 'bucket': 'arn:aws:s3:::sim2real-s3files-ghost-a58b76949f0d', 'prefix': '', 'clientToken': '...', 'status': 'creating', 'roleArn': 'arn:aws:iam::000000000000:role/sim2real-s3files-role-...', 'ownerId': '000000000000'}

Harness assertion: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {...} (check line 27 -> _error() line 54). Generated ids, account, region and timestamps above are environment-specific and are not part of the requirement; the 404 + ResourceNotFoundException classification is.

## Steps to Reproduce
1. Create an IAM role usable by s3files (any ARN matching the modeled RoleArn pattern ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/...$).
2. Pick a bucket name that was never created, and form a well-formed ARN for it: bucket_arn = 'arn:aws:s3:::<never-created-name>'.
3. Call s3files CreateFileSystem(bucket=bucket_arn, roleArn=<role arn>).
   Expected: ClientError with Error.Code 'ResourceNotFoundException', HTTP status 404, and nothing created.
   Observed: HTTP 201 with a populated CreateFileSystemResponse (status 'creating').
4. Call s3files ListFileSystems(bucket=bucket_arn). Expected an empty fileSystems list; the emulator has a file system recorded for the nonexistent bucket.
5. Replay: s3files.create-file-system-unknown-bucket-not-found.
