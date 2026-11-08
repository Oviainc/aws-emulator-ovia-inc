Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write
Issue-Key: autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write|_get_object#10|Timeout|return None
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# An ASG-launched instance's prefix-scoped instance role is never exercised: the scenario never converges within the suite budget

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
One net-new root cause.

Cause 1 of 1: an instance launched by an Auto Scaling group presents no running guest, so its instance-profile credentials never make the two S3 writes the scenario grades.
AWS: recorded behavior only — EC2's model carries LaunchTemplateData.UserData as an opaque base64 blob and says nothing about execution. In the recording, the ASG instance boots, reads IMDSv2 for its instance id, uses its instance-profile credentials to PUT s3://<bucket>/denied/id (refused AccessDenied by the role, which allows s3:PutObject only on <bucket>/allowed/*), and then PUTs the allowed marker object carrying the error code it observed; the allowed object's presence is what proves the forbidden attempt already happened and the forbidden key's absence is evidence.
Emulator: neither object ever appears and no API call reports an error; the check makes no progress and ends "Check timed out after 180.0s". Nothing behind the ASG-launched instance runs, so the instance-role scoping is unobservable.
Why: a client cannot distinguish "the role's prefix scoping denied the write" from "nothing ran" — every instance-identity and instance-profile scoping scenario silently hangs instead of either succeeding or being denied.
Fix: the EC2 instance runtime produced by the Auto Scaling launch path — guest boot, UserData execution, and instance-profile credential delivery through IMDS — the same instance materialisation RunInstances owns; S3 and IAM are only the observation points. The open issue 20261107T075601Z-autoscaling-ec2-s3-an-asg-launched-instance-never-boots-a-guest.md describes this same symptom for this check but no longer carries its replay key (its recorded key is the 60 s timeout at _poll#10, while the check now ends at the suite's 180 s cap), and 20261107T124656Z-autoscaling-ec2-s3-an-asg-launched-instance-never-runs-its-userdata.md carries only the sibling identity-write check; this filing re-attaches the key so the fix is gated on it.
Traces: autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write.

## Evidence
From the behavioral replay, autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write [mismatch]:

Error: Check timed out after 180.0s

Expected: after the ASG launches its instance from a launch template whose UserData performs the two writes, the object <bucket>/allowed/<marker> exists and reports the error code AccessDenied for the forbidden PUT, and <bucket>/denied/id does not exist (bucket name, prefixes and instance id are per-run values; the requirement is that the guest runs, is denied on the unscoped prefix and succeeds on the scoped one).

Actual: no object is ever written, no API call returns an error, and the check never reaches any assertion before the budget ends.

## Steps to Reproduce
1. Boot the emulator. Create an S3 bucket and an IAM role + instance profile whose policy allows s3:PutObject only on arn:aws:s3:::<bucket>/allowed/*.
2. CreateLaunchTemplate with that IamInstanceProfile, an egress-only security group and base64 UserData that: reads IMDSv2 for the instance id, PUTs <bucket>/denied/id (expected AccessDenied), captures the error code, then PUTs the allowed marker under <bucket>/allowed/.
3. CreateAutoScalingGroup from that template with DesiredCapacity=1 over usable subnets.
4. Poll s3:HeadObject/GetObject for the allowed marker and for <bucket>/denied/id.
5. Observe neither key ever appears — no guest runs behind the launched instance — while every control-plane call returns success.
6. Re-replay autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write to confirm the fix.
