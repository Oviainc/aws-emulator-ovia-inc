Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle
Issue-Key: ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 6596661b7bed7b2c68eacfc83c0e7a703969540b

# ec2__ssm: a CloudFormation stack for an SSM-managed public-subnet host ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `6596661b7bed7b2c68eacfc83c0e7a703969540b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a stack describing a standard SSM-managed EC2 host cannot be created — it rolls back instead of reaching CREATE_COMPLETE, so the whole scenario fails in its first phase.
AWS: CreateStack on a template containing AWS::EC2::VPC, InternetGateway + VPCGatewayAttachment, a public Subnet, RouteTable + Route (0.0.0.0/0 -> IGW) + SubnetRouteTableAssociation, two AWS::EC2::SecurityGroup, an AWS::S3::Bucket, an AWS::IAM::Role with the AmazonSSMManagedInstanceCore managed policy plus inline policies, an AWS::IAM::InstanceProfile, and an AWS::EC2::Instance whose ImageId comes from a parameter of type AWS::SSM::Parameter::Value<AWS::EC2::Image::Id> (default /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64) reaches CREATE_COMPLETE and publishes its Outputs (instance id, subnet, route table, IGW, group ids, role and profile names, bucket name). Recorded behavior only; the CFN model states shapes, not which resource types resolve.
Emulator: the stack reaches ROLLBACK_COMPLETE — 'stack ... reached \'ROLLBACK_COMPLETE\', not CREATE_COMPLETE — read its stack events for the resource that failed' — in phase 1 of 4, so no output of the estate is ever available and nothing downstream (the SSM session/SSH access-path derivation) is exercised. The failing resource is not named in the replay output. Hypothesis: the SSM-parameter-typed template parameter (AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>) or one of the IAM/S3 resource providers in the stack fails to resolve and takes the stack into rollback.
Why: a client cannot deploy the single most common EC2+SSM reference stack. Every resource it declares is rolled back, so the estate is unusable and the failure surfaces only as a rollback with no actionable refusal on any one API call.
Fix: the stack-execution path for this resource set — in particular the resolution of AWS::SSM::Parameter::Value<AWS::EC2::Image::Id> parameters against the public SSM parameter namespace, and the IAM role/instance-profile and S3 bucket providers — must succeed so the stack reaches CREATE_COMPLETE; where a resource genuinely cannot be created, its stack event must name it.
Traces: ec2__ssm.e2e-trace-ec2-ssh-access-path-lifecycle. The open issue 20261103T200742Z-ec2-ssm-cloudformation-deployed-ec2-ssm-estates-are-unusable-instance.md reports a different stage of the EC2+SSM deploy story and does not carry this check's replay key; this check is not marked OWNED by the replay report.

## Evidence
Expected: CreateStack of the carved ec2-ssh-access template (VPC/IGW/subnet/route/2 security groups/S3 bucket/IAM role+instance profile/EC2 instance with an AWS::SSM::Parameter::Value<AWS::EC2::Image::Id> AMI parameter) reaches CREATE_COMPLETE and DescribeStacks returns its Outputs. Actual: 'AssertionError: [phase 1/4 deploy] stack <name> reached \'ROLLBACK_COMPLETE\', not CREATE_COMPLETE — read its stack events for the resource that failed' (the stack name is a per-run value).

## Steps to Reproduce
1. Replay the behavioral suite and read the failure under '### ec2__ssm'.
2. CreateStack with the template above, passing an assets bucket name, an AZ that offers t3.micro, and leaving InstanceAmi at its /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 SSM-parameter default.
3. Poll DescribeStacks: the stack settles ROLLBACK_COMPLETE; DescribeStackEvents to see which resource failed.
