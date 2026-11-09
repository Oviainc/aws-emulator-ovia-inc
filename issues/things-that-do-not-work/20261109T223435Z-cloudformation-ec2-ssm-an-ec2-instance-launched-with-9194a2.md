Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance
Issue-Key: cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance|_wait_ssm_online#17|AssertionError|raise AssertionError(f"{instance_id} never came Online in SSM within {timeout_s}s: {last!r}")
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__ec2__ssm: an EC2 instance launched with an SSM instance profile never becomes Online in Systems Manager

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: an instance CloudFormation launches with an SSM-capable instance profile never registers with Systems Manager.
AWS: an EC2 instance with an instance profile granting AmazonSSMManagedInstanceCore registers with Systems Manager shortly after boot; DescribeInstanceInformation (filtered on that InstanceId) then returns an InstanceInformationList entry whose PingStatus is `Online`.
Emulator: DescribeInstanceInformation keeps returning an empty list for the instance id for the whole polling window, so the instance is never Online and nothing downstream (SendCommand, inventory, associations) can target it.
Why: a client that deploys an EC2+SSM estate with CloudFormation cannot manage the instance it just created; every SSM operation against it fails for want of a registered managed instance.
Fix: the SSM managed-instance registration that must follow EC2 RunInstances when the instance carries an SSM-capable instance profile — the SSM state's instance-information store.
Traces: the failing check below.

## Evidence
AssertionError: i-00000000000000001 never came Online in SSM within 47.6s: [] — DescribeInstanceInformation returned an empty InstanceInformationList. Expected an entry for that InstanceId with PingStatus 'Online'.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Deploy a stack with an EC2 instance plus an IAM instance profile granting AmazonSSMManagedInstanceCore.
3. Poll DescribeInstanceInformation filtered on that InstanceId.
