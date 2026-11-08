Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-instance-refresh-replaces-ec2-instances
Issue-Key: autoscaling__ec2__elbv2.asg-instance-refresh-replaces-ec2-instances|run#24|AssertionError|assert inst["State"]["Name"] == "terminated", "original instance must have been terminated by the refresh"
Issue-Commit: bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c

# StartInstanceRefresh reports Successful without ever terminating the instances it replaced

> _Note: this issue was identified against the code base at commit `bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceRefresh

## Description
One net-new root cause, grounded in one failing behavioral check.

Cause 1 of 1: an instance refresh surges a replacement instance and then reports Status "Successful", but the original instance it was supposed to replace is left alive instead of being terminated.
AWS: Smithy: com.amazonaws.autoscaling#InstanceRefresh.Status documents "Successful - An instance refresh completed successfully", and com.amazonaws.autoscaling#InstanceRefresh.Strategy documents the default strategy "Rolling - Terminates instances and launches replacements in batches". Recorded behavior: with Preferences {MinHealthyPercentage: 100, MaxHealthyPercentage: 200, InstanceWarmup: 0} on a group of DesiredCapacity 1 / MaxSize 2, the refresh launches the replacement on the new launch-template version, then terminates the original; once Status is Successful the group holds exactly one InService instance whose id is disjoint from the original set, and ec2 DescribeInstances on the original id reports State.Name == "terminated". A refresh that has not finished terminating its replaced instances is not yet Successful.
Emulator: the refresh reaches Status "Successful" and the replacement is InService on the new version, but the original instance is still in a non-terminated state when DescribeInstances is asked about it. Replay detail: AssertionError "original instance must have been terminated by the refresh" on inst["State"]["Name"] == "terminated"; the earlier assertions (Status Successful, one InService instance, the new id disjoint from the old, the replacement's InstanceType being the new version's) all passed.
Why: the refresh leaks the instances it claims to have replaced. A client that waits for Successful and then counts or bills the group's EC2 footprint sees double capacity forever; drain/replace automation that keys off the old instance being terminated never completes, and the group's real capacity permanently exceeds DesiredCapacity.
Fix: the StartInstanceRefresh execution path in the Auto Scaling group's instance-refresh state machine - the step that must terminate each replaced instance before the refresh is allowed to transition to Successful.
Traces: autoscaling__ec2__elbv2.asg-instance-refresh-replaces-ec2-instances.

## Evidence
Check [mismatch] autoscaling__ec2__elbv2.asg-instance-refresh-replaces-ec2-instances
  Expected (AWS): after DescribeInstanceRefreshes reports Status "Successful", ec2 DescribeInstances on the instance id the group held BEFORE the refresh returns State.Name == "terminated".
  Actual (emulator): that instance is not terminated - AssertionError "original instance must have been terminated by the refresh". Status "Successful", the single new InService id (disjoint from the old) and its InstanceType t3.small were all correct. (Instance ids are per-run values; the requirement is that every pre-refresh instance ends terminated.)

## Steps to Reproduce
1. Create a security group and a launch template version 1 with InstanceType t3.micro on an AL2023 AMI.
2. autoscaling CreateAutoScalingGroup(LaunchTemplate={LaunchTemplateId: LT, Version: "1"}, MinSize=1, MaxSize=2, DesiredCapacity=1, VPCZoneIdentifier=<subnet>); wait for one InService instance and record its id as OLD.
3. ec2 CreateLaunchTemplateVersion(LaunchTemplateId=LT, SourceVersion="1", LaunchTemplateData={InstanceType: t3.small}); autoscaling UpdateAutoScalingGroup(LaunchTemplate={LaunchTemplateId: LT, Version: "2"}).
4. autoscaling StartInstanceRefresh(Preferences={MinHealthyPercentage: 100, MaxHealthyPercentage: 200, InstanceWarmup: 0}) and poll DescribeInstanceRefreshes until Status is terminal.
   Observed: Status "Successful".
5. ec2 DescribeInstances(InstanceIds=[OLD]).
   Observed: State.Name is not "terminated" - the replaced instance survives the refresh. AWS reports it terminated.
