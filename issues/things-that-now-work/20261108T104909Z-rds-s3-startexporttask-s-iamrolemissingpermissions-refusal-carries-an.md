Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative
Issue-Key: rds__s3|StartExportTask|IamRoleMissingPermissions
Issue-Commit: b173a8e674d7ba975def7d68c06d3de96ae43616

# rds__s3: StartExportTask's IamRoleMissingPermissions refusal carries an emulator-authored sentence instead of AWS's fixed message

> _Note: this issue was identified against the code base at commit `b173a8e674d7ba975def7d68c06d3de96ae43616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
Cause 1 of 1: when the export role cannot write the destination bucket (here because the bucket policy Denies it), RDS correctly refuses StartExportTask synchronously with IamRoleMissingPermissions, but the message is the emulator's own prose rather than AWS's wording.
AWS: StartExportTask validates the export role's S3 write access on the destination before accepting the task and refuses with 400 IamRoleMissingPermissions (Smithy com-amazonaws-rds-amazonrdsv19: StartExportTask errors include IamRoleMissingPermissionsFault, errorKind client, httpError 400, awsQueryError code IamRoleMissingPermissions). The message is a single fixed sentence naming the role and the bucket: "The IAM role <IamRoleArn> isn't authorized to call s3:PutObject on the S3 bucket <S3BucketName>." — fixed text is "The IAM role ", " isn't authorized to call s3:PutObject on the S3 bucket " and the terminating "."; the two variable parts are the request's IamRoleArn and S3BucketName.
Emulator: the status (400) and the code (IamRoleMissingPermissions) match, but the message is replaced by "The IAM role <IamRoleArn> requires additional permissions to export to the Amazon S3 bucket <S3BucketName>: the role's policy must allow s3:PutObject on the objects of that bucket, and the bucket policy must not deny it." — an explanation AWS never emits.
Why: a client that matches, logs or branches on the refusal text (or renders it to an operator) sees a message that does not exist in AWS, and the diagnosis it prints names a different permission model than the one AWS reports; message-equality assertions and support runbooks keyed on AWS's sentence break.
Fix: the StartExportTask handler's role/destination authorization refusal in the rds service crate — the message template built for IamRoleMissingPermissionsFault.
Traces: rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative.

## Evidence
Request: StartExportTask with a snapshot SourceArn, S3BucketName = the bucket whose policy Denies the role, IamRoleArn = the export role, KmsKeyId = the CMK.
Expected (AWS): HTTP 400, Error.Code = IamRoleMissingPermissions, Error.Message = "The IAM role arn:aws:iam::<account>:role/<export-role> isn't authorized to call s3:PutObject on the S3 bucket <deny-bucket>."
Actual (emulator): HTTP 400, Error.Code = IamRoleMissingPermissions (correct), Error.Message = "The IAM role arn:aws:iam::<account>:role/<export-role> requires additional permissions to export to the Amazon S3 bucket <deny-bucket>: the role's policy must allow s3:PutObject on the objects of that bucket, and the bucket policy must not deny it."
The role ARN and bucket name are taken from the request; the account id, role name and bucket name seen in the recording are per-run values filling that template. The side effect is correct: no object is delivered to the denied bucket.

## Steps to Reproduce
1. Create an S3 bucket and attach a bucket policy with an explicit Deny of s3:PutObject for the export role.
2. Create a second (allow) bucket, an export role trusted by export.rds.amazonaws.com with s3:PutObject on both, and a KMS CMK.
3. Create a DB snapshot.
4. Call StartExportTask naming the DENY bucket, the role and the CMK.
5. Observe the ClientError: the code is IamRoleMissingPermissions as AWS returns, but the Message is the emulator's multi-clause sentence instead of AWS's "The IAM role <role> isn't authorized to call s3:PutObject on the S3 bucket <bucket>."
