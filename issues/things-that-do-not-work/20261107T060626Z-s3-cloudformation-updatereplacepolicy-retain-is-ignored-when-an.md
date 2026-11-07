Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.deploy-s3-1-rename-onto-owned-bucket-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.deploy-s3-1-rename-onto-owned-bucket-rejected|_wait_stack#34|AssertionError|raise AssertionError(
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# s3/cloudformation: UpdateReplacePolicy Retain is ignored when an AWS::S3::Bucket is replaced by a BucketName change — the old bucket is deleted, the delete fails "not empty", and the update rolls back

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: renaming the BucketName of an AWS::S3::Bucket that declares UpdateReplacePolicy: Retain replaces the resource and then DELETES the old bucket; because the old bucket holds objects the delete fails with S3's 409 BucketNotEmpty and the whole update rolls back, instead of creating the new bucket and leaving the old one behind untouched.

AWS: recorded behavior only for the provisioner semantics (CloudFormation resource attributes are template-level, not Smithy-modelled; the S3 refusal is S3's own). A BucketName change on AWS::S3::Bucket requires replacement; with UpdateReplacePolicy: Retain, CloudFormation creates the bucket under the new name, repoints the logical resource at it, and RETAINS the old bucket — it issues no DeleteBucket for it, so its objects survive and remain readable after the update. The stack therefore reaches UPDATE_COMPLETE, DescribeStackResource reports the new name as the Bucket PhysicalResourceId, the new bucket carries the aws:cloudformation:stack-name tag, and GetObject on the retained old bucket still returns its bytes. (S3 itself refuses DeleteBucket on a non-empty bucket with HTTP 409, error code BucketNotEmpty, message "The bucket you tried to delete is not empty" — which is exactly why Retain exists and why AWS must not attempt that delete.)

Emulator: the update ends UPDATE_ROLLBACK_COMPLETE. The events show the replacement's cleanup being attempted anyway: Bucket DELETE_FAILED with reason `Resource handler returned message: "The bucket you tried to delete is not empty (Service: S3, Status Code: 409, Request ID: <id>, Extended Request ID: <id>) (SDK Attempt Count: 3)" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException)`, followed by the stack-level UPDATE_FAILED 'The following resource(s) failed to update: [Bucket]. '. In that reason the fixed text is the envelope plus S3's "The bucket you tried to delete is not empty"; the request ids, extended request id and request token are per-run values. The defect is that the delete is issued at all despite UpdateReplacePolicy: Retain.

Why: UpdateReplacePolicy: Retain is the standard guard that keeps data alive across a bucket rename, and it is exactly what a client relies on when a rename is part of a migration. In the emulator any such rename of a non-empty bucket fails the whole stack update and rolls back, so a template AWS deploys cleanly cannot be deployed at all — and a client that reads the failure would conclude its data blocked the change, when AWS would never have touched it.

Fix: the CloudFormation stack provisioner's replacement/cleanup step — it must honour the resource's UpdateReplacePolicy (Retain/Snapshot/Delete) and skip the DeleteBucket on the replaced AWS::S3::Bucket when Retain is declared.

Traces: s3.deploy-s3-1-rename-onto-owned-bucket-rejected (the positive control at the end of the check: renaming the same stack's bucket onto a free name).

## Evidence
Check s3.deploy-s3-1-rename-onto-owned-bucket-rejected [mismatch].

Expected: after the control UpdateStack sets BucketNameParam to a free name, DescribeStacks(stack).StackStatus == 'UPDATE_COMPLETE'; DescribeStackResource('Bucket').PhysicalResourceId == the new name; the new bucket's tag set contains 'aws:cloudformation:stack-name'; and GetObject(Bucket=<original retained bucket>, Key='doc.txt') still returns b'stack-data'.

Actual: AssertionError — "stack <stack> reached 'UPDATE_ROLLBACK_COMPLETE', expected 'UPDATE_COMPLETE'; failures: [(<stack>, 'UPDATE_FAILED', 'The following resource(s) failed to delete: [Bucket]. '), ('Bucket', 'DELETE_FAILED', 'Resource handler returned message: \"The bucket you tried to delete is not empty (Service: S3, Status Code: 409, Request ID: <id>, Extended Request ID: <id>) (SDK Attempt Count: 3)\" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException)'), (<stack>, 'UPDATE_FAILED', 'The following resource(s) failed to create: [Bucket]. '), ('Bucket', 'CREATE_FAILED', 'Resource handler returned message: \"<owned-bucket-name> already exists ...\" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)')]" — the last two entries belong to the earlier, correctly-rejected rename onto an already-owned name; the first two are the control rename, where the retained bucket was deleted anyway.

## Steps to Reproduce
1) CreateBucket a bucket <squat> (outside the stack) and PutObject squat.txt into it.
2) CreateStack with a template whose only resource is `Bucket: {Type: AWS::S3::Bucket, UpdateReplacePolicy: Retain, Properties: {BucketName: !Ref BucketNameParam}}`, BucketNameParam=<orig>; wait CREATE_COMPLETE; PutObject doc.txt into <orig>.
3) UpdateStack (UsePreviousTemplate=True) with BucketNameParam=<squat>; it correctly rolls back to UPDATE_ROLLBACK_COMPLETE.
4) UpdateStack (UsePreviousTemplate=True) with BucketNameParam=<fresh>, a name nobody owns, and poll DescribeStacks: instead of UPDATE_COMPLETE the stack reaches UPDATE_ROLLBACK_COMPLETE, with a Bucket DELETE_FAILED event reporting S3's 409 "The bucket you tried to delete is not empty" for the retained bucket <orig>.
