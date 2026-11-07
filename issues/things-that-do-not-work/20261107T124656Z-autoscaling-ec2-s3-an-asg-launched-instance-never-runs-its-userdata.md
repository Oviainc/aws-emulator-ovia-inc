Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__s3.asg-launched-instance-boots-and-writes-its-own-identity
Issue-Key: autoscaling__ec2__s3.asg-launched-instance-boots-and-writes-its-own-identity|_poll#13|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: ee9893ed79b0abeb742411b16639d26129234894

# An ASG-launched instance never runs its UserData, so it never writes its own identity to S3 (1 cause)

> _Note: this issue was identified against the code base at commit `ee9893ed79b0abeb742411b16639d26129234894`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Cause 1 of 1 — an instance launched by an Auto Scaling group from a launch template that carries UserData never executes that UserData, so the object the guest is supposed to write never appears in S3.
AWS: recorded behavior only — EC2's Smithy model carries LaunchTemplateData.UserData as an opaque base64 blob and says nothing about execution, and Auto Scaling's model only describes the group. In the recording, a group created from a launch template whose UserData writes the instance's own identity launches an instance that boots, assumes its instance profile, and PUTs s3://<bucket>/<prefix>/id; GetObject for that key then returns the instance's id.
Emulator: the group is created and the check polls S3 for the object, which never appears; the check ends with `TimeoutError: the launched instance wrote s3://<bucket>/<prefix>/id from its UserData: the run budget ended first; not graded`. No guest is ever booted behind the ASG-launched instance, so neither the UserData nor the instance-profile credential path runs.
Why: any scenario that depends on an ASG instance doing work — bootstrapping an agent, registering itself, writing a heartbeat or an artifact, or proving its instance role's permissions — cannot be emulated at all; a client sees an instance it believes is InService but which never acts, and there is no error to key on.
Fix: the EC2 instance launch/guest component reached through the Auto Scaling launch path (UserData execution and the instance-profile credential plumbing for ASG-launched instances), not the S3 handler — S3 is only the observation point. The open issue 20261107T075601Z-autoscaling-ec2-s3-an-asg-launched-instance-never-boots-a-guest.md describes this same symptom for the sibling permission-scoping check but does not carry this check's replay key, so this check is still unowned.
Traces: autoscaling__ec2__s3.asg-launched-instance-boots-and-writes-its-own-identity.

## Evidence
Expected: after the ASG launches its instance, the object <prefix>/id exists in the check's bucket and its body is the launched instance's own id (the bucket name, prefix and instance id are per-run values; the requirement is that the UserData-written object appears).

Actual (suite verdict `error`):
File "check_autoscaling__ec2__s3_asg_launched_instance_boots_and_writes_its_own_identity.py", line 72, in run -> _poll(...) -> line 134 raise TimeoutError
TimeoutError: the launched instance wrote s3://sim2real-xsvc-boot-<id>/sim2real-xsvc-boot-<id>/id from its UserData: the run budget ended first; not graded

No S3 object is ever created and no error is returned by any API call — the launch simply has no guest behind it.

## Steps to Reproduce
1. Boot the emulator. Create an S3 bucket and an IAM role/instance profile allowing s3:PutObject on <bucket>/<prefix>/*.
2. CreateLaunchTemplate with that IamInstanceProfile and base64 UserData that writes the instance's own instance id to s3://<bucket>/<prefix>/id.
3. CreateAutoScalingGroup from that launch template with DesiredCapacity=1.
4. Poll s3:GetObject(<bucket>, <prefix>/id).
5. Observe the key never appears (AWS returns the launched instance's id), while no API call reports an error.
6. Re-replay autoscaling__ec2__s3.asg-launched-instance-boots-and-writes-its-own-identity to confirm a fix.
