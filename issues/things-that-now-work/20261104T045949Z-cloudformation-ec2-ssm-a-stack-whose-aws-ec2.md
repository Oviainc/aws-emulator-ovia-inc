Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cfn-stack-rollback-on-invalid-ec2-instance-type-negative
Issue-Key: cloudformation__ec2__ssm.cfn-stack-rollback-on-invalid-ec2-instance-type-negative|run#5|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudformation__ec2__ssm: a stack whose AWS::EC2::Instance has an invalid InstanceType reaches CREATE_COMPLETE instead of CREATE_FAILED/ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause.

Cause 1 of 1 — CloudFormation provisions an AWS::EC2::Instance with a syntactically valid but nonexistent InstanceType and reports the stack created, instead of failing that resource and rolling the stack back.
AWS: recorded behavior only for the CloudFormation status machine (the EC2 Smithy model carries no CFN coupling). In AWS, provisioning AWS::EC2::Instance calls EC2 RunInstances, which refuses an unknown instance type with a client error naming the offending value (AWS's wording for this class is InvalidParameterValue, "Invalid value '<the InstanceType from the request>' for InstanceType." — fixed text "Invalid value '" / "' for InstanceType." with the request's InstanceType as the only variable part). CloudFormation then records a CREATE_FAILED stack event for that logical resource whose ResourceStatusReason carries that EC2 message (so the failure is attributable to InstanceType and not merely to "something failed"), skips the dependent AWS::SSM::Association entirely — it emits NO stack event for the dependent logical id — leaves no EC2 instance behind, and the stack ends in ROLLBACK_COMPLETE. A sibling stack differing only in InstanceType reaches CREATE_COMPLETE.
Emulator: the stack reaches CREATE_COMPLETE ("AssertionError: expected ROLLBACK_COMPLETE, got CREATE_COMPLETE") — the invalid InstanceType is accepted, the instance resource never fails, and no rollback occurs.
Why: a template that cannot work in AWS deploys "successfully" here, so a user validating a template or a CI gate that asserts on rollback gets a false pass; and because the resource never fails, the dependent-resource-skip and no-orphan-instance guarantees that follow a failed create cannot hold either.
Fix: the AWS::EC2::Instance resource provider's property validation / its RunInstances call path (an unknown InstanceType must be refused by EC2 and surfaced as the resource's failure reason), plus the CloudFormation create path that must mark the resource CREATE_FAILED with that reason, skip dependents and drive the stack to ROLLBACK_COMPLETE.
Traces: cloudformation__ec2__ssm.cfn-stack-rollback-on-invalid-ec2-instance-type-negative.

## Evidence
Expected: CreateStack over a template with Resources {TheInstance: AWS::EC2::Instance with InstanceType set to a nonexistent type and ImageId from an AWS::SSM::Parameter::Value<AWS::EC2::Image::Id> parameter, TheAssociation: AWS::SSM::Association targeting {"Ref": "TheInstance"}} settles at ROLLBACK_COMPLETE, DescribeStackEvents shows a CREATE_FAILED event for TheInstance whose ResourceStatusReason names the invalid InstanceType, shows no event at all for TheAssociation, and DescribeInstances filtered on the stack's aws:cloudformation:stack-name tag returns no reservations.
Actual: the stack's terminal status is CREATE_COMPLETE — "AssertionError: expected ROLLBACK_COMPLETE, got CREATE_COMPLETE". (The instance type used in the recording is one example value; any nonexistent type must be refused.)

## Steps to Reproduce
1. cloudformation CreateStack with Parameters {ImageId: {Type: "AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>", Default: "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64"}}, Resources {TheInstance: {Type: "AWS::EC2::Instance", Properties: {ImageId: {Ref: ImageId}, InstanceType: "sim2real.invalid-type" (any nonexistent type), SubnetId: <a subnet>}}, TheAssociation: {Type: "AWS::SSM::Association", Properties: {Name: "AWS-RunShellScript", Targets: [{Key: "InstanceIds", Values: [{Ref: "TheInstance"}]}]}}}, Outputs {InstanceId: {Value: {Ref: TheInstance}}}.
2. Poll DescribeStacks until the stack reaches a terminal status.
3. Observe CREATE_COMPLETE instead of ROLLBACK_COMPLETE; DescribeStackEvents shows no CREATE_FAILED event for TheInstance naming the InstanceType.
4. Create a sibling stack differing only in a valid InstanceType and confirm it also reaches CREATE_COMPLETE — the two are indistinguishable.
