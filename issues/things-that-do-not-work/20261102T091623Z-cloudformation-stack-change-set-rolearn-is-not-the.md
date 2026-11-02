Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sqs__ssm.cdk-deploy-role-arn-stays-the-stacks-role-for-later-deploys cloudformation__iam__sqs__ssm.change-set-role-arn-is-the-acting-principal cloudformation__iam__ssm.change-set-role-arn-on-update-is-kept-for-later-change-sets cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke codecommit__codepipeline__glue__iam__lambda.e2e-diagnose-glue-pipeline-not-running-lifecycle iam__s3.bap-e2e-check-s-bucket-access-profiles-lifecycle cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative cloudformation__iam__ssm.stackset-managed-execution-accepts-a-conflicting-update-and-runs-it-after cloudformation__iam__ssm.stackset-parameter-overrides-survive-a-stack-set-parameter-update
Issue-Key: cloudformation__iam__sqs__ssm.cdk-deploy-role-arn-stays-the-stacks-role-for-later-deploys|run#28|AssertionError|assert rc == 1, f"the deploy adding a queue exited {rc}; output tail {out[-800:]!r}"
Issue-Key: cloudformation__iam__sqs__ssm.change-set-role-arn-is-the-acting-principal|run#11|AssertionError|assert (stack["StackStatus"], stack.get("RoleARN")) == ("ROLLBACK_COMPLETE", narrow["arn"]), (
Issue-Key: cloudformation__iam__ssm.change-set-role-arn-on-update-is-kept-for-later-change-sets|run#22|AssertionError|assert stack["StackStatus"] == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke|run#13|WaiterError|awslambda.get_waiter("function_active_v2").wait(FunctionName=function_name, WaiterConfig={"Delay": 2, "MaxAttempts": 150})
Issue-Key: codecommit__codepipeline__glue__iam__lambda|GetFunction|ResourceNotFoundException
Issue-Key: iam__s3|GetBucketPolicy|NoSuchBucketPolicy
Issue-Key: cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative|run#18|AssertionError|assert got == want, f"{missing_url!r}: expected {want!r}, got {got!r}"
Issue-Key: cloudformation|UpdateStackSet|OperationInProgressException
Issue-Key: cloudformation__iam__ssm.stackset-parameter-overrides-survive-a-stack-set-parameter-update|run#20|AssertionError|assert got == (
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# cloudformation: stack/change-set RoleARN is not the acting principal, declared Lambda functions and bucket policies are never materialized, and stack-set update semantics diverge

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
This issue carries 6 root causes in CloudFormation. The open issues 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md, 20261029T072657Z-cloudformation-iam-lambda-logs-a-stack-declaring-aws-e5cb3e.md, 20261029T192356Z-cloudformation-iam-s3-createchangeset-with-a-templateurl-never.md and 20261029T174654Z-cloudwatch-dynamodb-ecr-events-iam-logs-s3-bucket-policy-does-not-round-trip-getbucketpolicy.md describe neighbouring symptoms but carry only other checks' replay keys; the checks below are additional.

Cause 1 of 6: a stack's (or change set's) RoleARN is recorded but never used as the acting principal.
AWS: when CreateStack/UpdateStack/CreateChangeSet carries RoleARN (Smithy: CreateChangeSet.RoleARN, length 20-2048), CloudFormation performs every resource operation with THAT service role, not with the caller's permissions, and keeps it for later operations on the stack. A stack whose role cannot perform a declared write therefore ends CREATE/UPDATE in a failure state — CREATE/ROLLBACK_COMPLETE on create, UPDATE_ROLLBACK_COMPLETE on update — and DescribeStacks reports the role in RoleARN.
Emulator: the writes are performed with the caller's own permissions, so a stack whose narrow service role is not allowed the write reaches CREATE_COMPLETE / UPDATE_COMPLETE (and a CDK deploy that must fail exits 0).
Why: a least-privilege deployment role is not a control — a template that real AWS refuses to deploy succeeds here, so an operator cannot validate a deploy role at all, and a rollback path is never exercised.
Fix: the stack and change-set execution path must assume the stack's RoleARN for resource provider calls and keep it across subsequent operations.
Traces: cloudformation__iam__sqs__ssm.cdk-deploy-role-arn-stays-the-stacks-role-for-later-deploys, cloudformation__iam__sqs__ssm.change-set-role-arn-is-the-acting-principal, cloudformation__iam__ssm.change-set-role-arn-on-update-is-kept-for-later-change-sets.

Cause 2 of 6: a stack declaring AWS::Lambda::Function reaches CREATE_COMPLETE without the function existing.
AWS: once a stack is CREATE_COMPLETE and DescribeStackResources lists an AWS::Lambda::Function, that function exists in Lambda under the name/physical id the stack reports, so GetFunction and the FunctionActiveV2 waiter succeed (Smithy: Lambda GetFunction declares ResourceNotFoundException with HTTP 404 only for a function that does not exist).
Emulator: the stack reports CREATE_COMPLETE and owns 'AWS::IAM::Role' and 'AWS::Lambda::Function', but Lambda answers ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>' for that same name, both for an explicitly named function and for a CFN-generated one.
Why: every downstream step — waiting for the function to go Active, invoking it, reading its log group, diagnosing a pipeline that calls it — fails against a stack the emulator declares complete.
Fix: the AWS::Lambda::Function resource provider must actually create the function (and register its physical resource id) before the stack reaches CREATE_COMPLETE.
Traces: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke, codecommit__codepipeline__glue__iam__lambda.e2e-diagnose-glue-pipeline-not-running-lifecycle.

Cause 3 of 6: a stack declaring AWS::S3::BucketPolicy never applies the policy to the bucket.
AWS: after a stack with an AWS::S3::BucketPolicy resource reaches CREATE_COMPLETE, GetBucketPolicy on the referenced bucket returns the rendered PolicyDocument (intrinsics such as Fn::GetAtt resolved).
Emulator: GetBucketPolicy answers NoSuchBucketPolicy 'The bucket policy does not exist' for the bucket the same stack created.
Why: the bucket's declared Deny-insecure-transport and read grants are simply absent, so any workflow that reads the deployed policy back (as an access review does) fails and a bucket deployed as restricted is in fact unpoliced.
Fix: the AWS::S3::BucketPolicy resource provider must call PutBucketPolicy with the rendered document during stack create/update.
Traces: iam__s3.bap-e2e-check-s-bucket-access-profiles-lifecycle.

Cause 4 of 6: the ValidationError for a TemplateURL whose bucket does not exist has the wrong message.
AWS: CreateStack with a TemplateURL in a nonexistent bucket is refused ValidationError with 'S3 error: The specified bucket does not exist\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html' — entirely fixed text that quotes S3's own error, with no bucket or key in it.
Emulator: ValidationError 'S3 object does not exist. Bucket: <bucket>, Key: <key>. Error: NoSuchBucket'.
Why: a client that matches AWS's wording to distinguish a template-fetch failure from a template-content failure cannot, and the message exposes values AWS does not put there.
Fix: the template-fetch error mapping in the CreateStack/CreateChangeSet handler.
Traces: cloudformation__iam__lambda__s3.cfn-service-role-lacks-s3-getobject-for-template-negative.

Cause 5 of 6: UpdateStackSet with managed execution refuses a conflicting update instead of queueing it.
AWS: with ManagedExecution active (Smithy: UpdateStackSet.ManagedExecution), CloudFormation queues a conflicting stack-set operation and runs it after the one in flight, so the second UpdateStackSet is ACCEPTED and returns an OperationId; OperationInProgressException (Smithy-declared, HTTP 409) is the answer only when managed execution is NOT active.
Emulator: answers OperationInProgressException 'Another Operation on StackSet <stackset-arn> is in progress: <operation-id>'.
Why: a client relying on managed execution to serialize its updates gets a hard failure and must re-implement queueing.
Fix: the UpdateStackSet handler's in-flight-operation check must honour the stack set's ManagedExecution setting.
Traces: cloudformation__iam__ssm.stackset-managed-execution-accepts-a-conflicting-update-and-runs-it-after.

Cause 6 of 6: stack-instance parameter overrides are lost when the stack set's parameters are updated.
AWS: per-instance ParameterOverrides set with UpdateStackInstances survive a later UpdateStackSet that changes the stack set's own parameters; DescribeStackInstance keeps reporting each instance's overrides.
Emulator: after the stack-set parameter update one instance's ParameterOverrides reads as [] while the other keeps its override, so the overrides are not preserved per instance.
Why: an operator who pinned a region-specific value silently loses it and the instance drifts to the stack-set default.
Fix: the stack-set update path must carry existing per-instance ParameterOverrides into the regenerated instances.
Traces: cloudformation__iam__ssm.stackset-parameter-overrides-survive-a-stack-set-parameter-update.

## Evidence
Cause 1: "the ssm-only role's stack ended 'CREATE_COMPLETE' with RoleARN 'arn:aws:iam::000000000000:role/sim2real-cfn-exec-narrow-...'" where ROLLBACK_COMPLETE was required; "a write outside the role's policy must fail: 'UPDATE_COMPLETE'"; 'the deploy adding a queue exited 0' where exit 1 was required.
Cause 2: 'botocore.exceptions.WaiterError: Waiter FunctionActiveV2 failed: An error occurred (ResourceNotFoundException): Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-implfn-...' after the stack reached CREATE_COMPLETE owning [AWS::IAM::Role, AWS::Lambda::Function]; '[phase 2/4 solve] ResourceNotFoundException ... Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-refarch-glue-stack-...-EtlLaunchFunction-...'.
Cause 3: '[phase 3/5 solve] ClientError: An error occurred (NoSuchBucketPolicy) when calling the GetBucketPolicy operation: The bucket policy does not exist' for a stack whose template declares BucketPolicy of type AWS::S3::BucketPolicy.
Cause 4: "expected ('ValidationError', 'S3 error: The specified bucket does not exist\\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html'), got ('ValidationError', 'S3 object does not exist. Bucket: ..., Key: templates/service-role.json. Error: NoSuchBucket')".
Cause 5: 'botocore.errorfactory.OperationInProgressException: ... Another Operation on StackSet ... is in progress: ...'.
Cause 6: 'DescribeStackInstance ParameterOverrides for (us-west-2, us-east-1) are ([], [((ParameterKey, Note), (ParameterValue, override-...))])'.

## Steps to Reproduce
1. Cause 1: create a stack (or change set) with RoleARN set to a role that cannot perform one declared write and observe it reach CREATE_COMPLETE/UPDATE_COMPLETE instead of rolling back.
2. Cause 2: create a stack declaring AWS::IAM::Role + AWS::Lambda::Function with an explicit FunctionName, wait for CREATE_COMPLETE, then call lambda GetFunction / the function_active_v2 waiter on that name.
3. Cause 3: create a stack declaring AWS::S3::Bucket + AWS::S3::BucketPolicy and call GetBucketPolicy on the created bucket.
4. Cause 4: CreateStack with TemplateURL in a bucket that does not exist.
5. Cause 5: create a stack set with ManagedExecution active and issue a second UpdateStackSet while one is in flight.
6. Cause 6: set per-instance ParameterOverrides, then UpdateStackSet changing the stack set's parameters, then DescribeStackInstance for each region.
7. Or replay the check ids in trace_ids.
