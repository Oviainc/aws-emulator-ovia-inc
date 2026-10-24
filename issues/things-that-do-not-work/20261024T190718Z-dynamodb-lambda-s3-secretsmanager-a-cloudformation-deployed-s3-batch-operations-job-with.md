Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-role-trusts-s3-service-for-batch-operations
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-role-trusts-s3-service-for-batch-operations|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# A CloudFormation-deployed S3 Batch Operations job with a Lambda invoke never completes, so the batch role's service trust is never exercised

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the deploy-then-run S3 Batch Operations flow never reaches a terminal job state ===
AWS: a CloudFormation stack that provisions a bucket, a Lambda function and an IAM role trusting batchoperations.s3.amazonaws.com deploys to CREATE_COMPLETE; s3control CreateJob with LambdaInvoke then actually assumes that role, invokes the function once per object and completes the job - which the check verifies through the object tags the function applied, not the job status alone. Recorded behaviour of the end-to-end flow; no single Smithy shape states it.
Emulator: the flow does not converge - the check runs to the suite's 60 s cap ('Check timed out after 60.0s') while waiting on the deployment and the job, and the Lambda-applied object tags never appear. Hypothesis: the per-object LambdaInvoke cannot complete because the emulator has no runtime for a deployment package - open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records that lambda Invoke answers NotImplemented for a real package - and the job therefore never reports a terminal state.
Why: this is the only check that proves the service-principal trust (batchoperations.s3.amazonaws.com) is honoured end to end. A client cannot tell a correctly trusted role from an untrusted one, and a batch job that never terminates hangs any pipeline that waits on it.
Fix: the s3control CreateJob/DescribeJob execution path (drive the job to a terminal state and assume the supplied role) and the Lambda invocation runtime it depends on.
Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-role-trusts-s3-service-for-batch-operations.

## Evidence
'Error: Check timed out after 60.0s'. The check deploys the stack (bucket + python3.12 tagging Lambda + a role whose AssumeRolePolicyDocument trusts batchoperations.s3.amazonaws.com), then creates the S3 Batch job and verifies completion by reading the 'processed=true' tag the function applies to each object; no terminal job state or tag was ever observed inside the cap.

## Steps to Reproduce
cloudformation CreateStack with AWS::S3::Bucket, AWS::Lambda::Function (python3.12 inline ZipFile) and an AWS::IAM::Role trusting batchoperations.s3.amazonaws.com; put objects in the bucket; s3control CreateJob with Operation=LambdaInvoke pointing at the function and RoleArn=<batch ops role>; poll DescribeJob for a terminal status and s3 GetObjectTagging for the tag the function applies - neither converges.
