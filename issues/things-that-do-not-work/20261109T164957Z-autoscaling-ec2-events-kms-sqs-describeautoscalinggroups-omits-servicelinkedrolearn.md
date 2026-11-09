Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__events__kms__sqs.xsvc-autoscaling-2-disabled-cmk-launch-is-cancelled-and-evented-until-the-key-is-enabled
Issue-Key: autoscaling__ec2__events__kms__sqs.xsvc-autoscaling-2-disabled-cmk-launch-is-cancelled-and-evented-until-the-key-is-enabled|run#25|AssertionError|assert slr == (f"arn:aws:iam::{account}:role/aws-service-role/autoscaling.amazonaws.com/"
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# autoscaling__ec2__events__kms__sqs: DescribeAutoScalingGroups omits ServiceLinkedRoleARN, so a group created without one reports no service-linked role

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: an Auto Scaling group created without an explicit ServiceLinkedRoleARN is read back with that member absent instead of carrying the account's default Auto Scaling service-linked role ARN.
AWS: recorded behavior only for the default value (the model declares AutoScalingGroup.ServiceLinkedRoleARN as an optional member, so the model does not state the default). CreateAutoScalingGroup with no ServiceLinkedRoleARN binds the group to the account's AWSServiceRoleForAutoScaling service-linked role, and DescribeAutoScalingGroups reports ServiceLinkedRoleARN = arn:aws:iam::<account-id>:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling — fixed text apart from the account id, which comes from the caller's account.
Emulator: the member is absent from the described group (the SDK surfaces it as None), so the group names no role at all.
Why: the service-linked role ARN is what a client reads to know which identity Auto Scaling uses for the group — it is the identity a KMS key policy must grant for an encrypted-volume launch, and the value audits and templates compare against. Reading None, a client cannot construct that grant and cannot tell a default-role group from one with a custom role. The open issue 20261107T075601Z-autoscaling-ec2-kms-auto-scaling-group-reads-omit-servicelinkedrolearn-and-group.md reports the same omission but is bound to checks under the autoscaling__ec2__kms heading and does not carry this one.
Fix: the group projection in the DescribeAutoScalingGroups handler (and the default applied when CreateAutoScalingGroup stores a group with no ServiceLinkedRoleARN).
Traces: autoscaling__ec2__events__kms__sqs.xsvc-autoscaling-2-disabled-cmk-launch-is-cancelled-and-evented-until-the-key-is-enabled.

## Evidence
Expected ServiceLinkedRoleARN == f"arn:aws:iam::{account}:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling" on the described group; actual AssertionError: "unexpected ServiceLinkedRoleARN None" — the described group has no ServiceLinkedRoleARN member.

## Steps to Reproduce
1. CreateLaunchTemplate and CreateAutoScalingGroup with no ServiceLinkedRoleARN.
2. DescribeAutoScalingGroups for that group and read AutoScalingGroups[0].ServiceLinkedRoleARN — it must be arn:aws:iam::<account>:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling; the emulator omits the member.
