Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cfn-deploy-ssm-patch-baseline-associated-ec2-positive
Issue-Key: cloudformation__ec2__ssm.cfn-deploy-ssm-patch-baseline-associated-ec2-positive|_poll_until#9|Timeout|last = probe()
Issue-Commit: 6537d79e9f76a83fe65f7d2499927078c6ec62b1

# cloudformation__ec2__ssm: a CFN-deployed SSM patch baseline registered to a patch group on an SSM-managed EC2 instance never produces patch compliance, so the deployment never settles

> _Note: this issue was identified against the code base at commit `6537d79e9f76a83fe65f7d2499927078c6ec62b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstancePatchStates

## Description
Cause 1 of 1: the CloudFormation-deployed SSM patch-baseline estate never reaches the state that AWS reaches, so the check exhausts its budget without a terminal answer.

AWS: recorded behavior only. A CloudFormation stack that creates an AWS::IAM::Role with the SSM managed-instance trust, an instance profile, an EC2 instance whose ImageId comes from an AWS::SSM::Parameter::Value<AWS::EC2::Image::Id> parameter, an AWS::SSM::PatchBaseline and an AWS::SSM::PatchBaseline/patch-group registration reaches CREATE_COMPLETE; the instance then registers with SSM, AWS-RunPatchBaseline runs against the patch group, and the SSM patch APIs report compliance data for that instance. Every one of those steps answers within the deployment's lifetime.

Emulator: the check never gets a terminal answer — the replay reports 'Error: Check timed out after 180.0s'. The stack and/or the SSM-managed-instance registration and patch-compliance data never arrive, so the whole positive path (patch baseline -> patch group -> AWS-RunPatchBaseline -> compliance) is unobservable rather than wrong.

Why: a client deploying a patch-management estate through CloudFormation polls forever: it sees neither a completed deployment with compliance data nor a failure it can act on. Deployment tooling with a bounded wait treats this as an infrastructure outage.

Fix: the CloudFormation provisioners for AWS::SSM::PatchBaseline and the patch-group registration, together with the SSM managed-instance registration that the EC2 instance profile is supposed to trigger, and the SSM patch-compliance state the patch APIs read. The other 29 failures in this service group are bound to open issue 20261108T120914Z-cloudformation-ec2-ssm-cfn-deployed-ec2-ssm-estates.md (CFN-deployed EC2/SSM estates); the suite did not bind this check to it, so the patch-baseline leg is recorded here and should be closed alongside that issue.

Traces: cloudformation__ec2__ssm.cfn-deploy-ssm-patch-baseline-associated-ec2-positive

## Evidence
Check cloudformation__ec2__ssm.cfn-deploy-ssm-patch-baseline-associated-ec2-positive [mismatch]

expected: the stack reaches CREATE_COMPLETE, the EC2 instance becomes an SSM-managed instance, AWS-RunPatchBaseline runs for the patch group, and the SSM patch APIs return compliance data for the instance — all within the deployment's lifetime.
actual:   Error: Check timed out after 180.0s — no terminal state was reached at any step, so no value could be compared.

The check is the only one of the 30 failures under this service heading that the suite did not bind to the open CFN-deployed-EC2/SSM-estate issue, so its patch-baseline leg is currently unowned.

## Steps to Reproduce
cloudformation.CreateStack with:
  Parameters: {ImageId: {Type: 'AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>', Default: '/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64'}}
  Resources: an AWS::IAM::Role trusted by ssm.amazonaws.com with the SSM managed-instance policy, its AWS::IAM::InstanceProfile, an AWS::EC2::Instance using {Ref: ImageId} and that profile, an AWS::SSM::PatchBaseline, and the patch-group registration for a unique patch group name.

Then poll DescribeStacks for a terminal status and, once complete, call the SSM patch APIs (e.g. DescribeInstancePatchStatesForPatchGroup / DescribeInstancePatchStates) for the instance. The emulator reaches no terminal state within 180s.
