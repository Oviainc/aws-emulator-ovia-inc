Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lakeformation__s3.deploy-cfn-lakeformation-resource-invalid-role-arn-rejected cloudformation__iam__lakeformation__s3.deploy-cfn-lakeformation-resource-round-trip-and-delete
Issue-Key: cloudformation__iam__lakeformation__s3.deploy-cfn-lakeformation-resource-invalid-role-arn-rejected|run#12|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__iam__lakeformation__s3|ListResources|NotImplemented
Issue-Commit: e19d7761cdba7dfb41b7452b9b8e275a18501f91

# cloudformation__iam__lakeformation__s3: AWS::LakeFormation::Resource RoleArn is never pattern-validated (bad stack reaches CREATE_COMPLETE), and lakeformation ListResources is a 501 NotImplemented stub (2 causes)

> _Note: this issue was identified against the code base at commit `e19d7761cdba7dfb41b7452b9b8e275a18501f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the AWS::LakeFormation::Resource surface. Both must be closed before submitting.

Cause 1 of 2: the AWS::LakeFormation::Resource handler performs no property validation on RoleArn, so a stack whose RoleArn is not an IAM ARN is created instead of rejected.
AWS: the AWS::LakeFormation::Resource resource schema constrains RoleArn to the pattern ^arn:[a-z-]+:iam::([0-9]{12})?:.+$ (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-lakeformation-resource.html). A value missing that pattern fails CloudFormation's property validation before any resource of the stack is provisioned: the stack ends ROLLBACK_COMPLETE, DescribeStackResources reports no resources at all, and DescribeEvents records one VALIDATION_ERROR OperationEvent for the offending logical resource with ValidationName 'PROPERTY_VALIDATION', ValidationStatus 'FAILED', ValidationFailureMode 'FAIL', ValidationPath '/Resources/<LogicalId>/Properties/RoleArn' and ValidationStatusReason 'Property value [<the rejected value>] does not match pattern: ^arn:[a-z-]+:iam::([0-9]{12})?:.+$'. In that message 'Property value [' , '] does not match pattern: ' and the pattern itself are fixed text; the bracketed value is the RoleArn taken verbatim from the request. Because validation precedes provisioning, the stack's bucket and role never exist (s3 HeadBucket answers 404 'Not Found', iam GetRole answers NoSuchEntity 'The role with name <name> cannot be found.', lakeformation DescribeResource answers EntityNotFoundException 'Entity not found').
Emulator: the stack with RoleArn 'not-a-valid-role-arn' reaches StackStatus 'CREATE_COMPLETE' — the value is accepted, the resources are provisioned, and no VALIDATION_ERROR event is recorded.
Why: a client's malformed infrastructure is silently accepted. It believes an S3 location is registered with a usable IAM role when the recorded RoleArn cannot be assumed, so the failure moves from deploy time (where CloudFormation rolls back cleanly and names the offending property path) to first data access, and the client's own negative tests for template mistakes pass against the emulator and fail against AWS.
Fix: the AWS::LakeFormation::Resource handler's property validation in CloudFormation's resource-handler layer, plus the VALIDATION_ERROR OperationEvent emission that DescribeEvents reads, and the ordering guarantee that validation runs before any resource in the stack is created.
Traces: cloudformation__iam__lakeformation__s3.deploy-cfn-lakeformation-resource-invalid-role-arn-rejected.

Cause 2 of 2: lakeformation ListResources is an unimplemented scaffold stub.
AWS: Smithy: ListResources (POST /ListResources, 200) takes FilterConditionList/MaxResults/NextToken and returns ResourceInfoList plus NextToken; its only modeled errors are InternalServiceException (500), InvalidInputException (400) and OperationTimeoutException (400). A ListResources over an account where an S3 location has been registered reports a ResourceInfo whose ResourceArn is that location and whose RoleArn is the registering role; NotImplemented is not a modeled outcome.
Emulator: ListResources answers error code 'NotImplemented' with message 'operation ListResources is scaffolded but not implemented in this emulator'.
Why: the registered-locations inventory is unreadable. A client cannot enumerate Lake Formation registrations (to reconcile them with its templates, or to confirm a deploy), and because the error is an unmodeled code its SDK surfaces it as an unknown fault rather than a retryable or validation error.
Fix: implement the ListResources handler in the lakeformation service crate over the same registration state RegisterResource/DescribeResource use, including FilterConditionList filtering and NextToken paging.
Traces: cloudformation__iam__lakeformation__s3.deploy-cfn-lakeformation-resource-round-trip-and-delete.

## Evidence
Cause 1 — Expected: DescribeStacks(StackName=<bad stack>) StackStatus == 'ROLLBACK_COMPLETE'; DescribeEvents VALIDATION_ERROR rows == [('LFResource', 'PROPERTY_VALIDATION', 'FAILED', 'FAIL', 'Property value [not-a-valid-role-arn] does not match pattern: ^arn:[a-z-]+:iam::([0-9]{12})?:.+$', '/Resources/LFResource/Properties/RoleArn')]; DescribeStackResources == [].
Actual: AssertionError: rejected stack ended 'CREATE_COMPLETE'.

Cause 2 — Expected: lakeformation ListResources returns 200 with ResourceInfoList containing {ResourceArn: 'arn:<partition>:s3:::<bucket>', RoleArn: '<stack role arn>'} for the registered bucket.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListResources operation: operation ListResources is scaffolded but not implemented in this emulator

## Steps to Reproduce
Cause 1:
1. CreateStack (Capabilities=['CAPABILITY_NAMED_IAM']) with an AWS::S3::Bucket, an AWS::IAM::Role trusted by lakeformation.amazonaws.com, and {"Type": "AWS::LakeFormation::Resource", "DependsOn": ["ResourceRole", "Bucket"], "Properties": {"ResourceArn": {"Fn::Sub": "arn:${AWS::Partition}:s3:::${Bucket}"}, "RoleArn": "not-a-valid-role-arn", "UseServiceLinkedRole": false}}.
2. Poll DescribeStacks until terminal: it reports CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and DescribeEvents contains no VALIDATION_ERROR row for /Resources/LFResource/Properties/RoleArn.

Cause 2:
1. Deploy the same template with RoleArn = {"Fn::GetAtt": ["ResourceRole", "Arn"]} so the bucket is registered.
2. Call lakeformation ListResources (no arguments): it answers NotImplemented instead of the ResourceInfoList naming the registered bucket ARN and its role.
