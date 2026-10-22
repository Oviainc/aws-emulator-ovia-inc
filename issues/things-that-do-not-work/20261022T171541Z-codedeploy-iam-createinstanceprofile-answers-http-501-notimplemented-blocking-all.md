Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact codedeploy.ec2-agent-deployment-runs-appspec-hook-on-instance codedeploy.file-exists-behavior-disallow-blocks-foreign-file-and-overwrite-replaces-it codedeploy.hook-failure-skips-later-lifecycle-events-and-fails-the-target codedeploy.lifecycle-event-order-and-applicationstop-only-on-redeploy codedeploy.minimum-healthy-hosts-bounds-observed-concurrent-unavailability
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|CreateInstanceProfile|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# IAM CreateInstanceProfile answers HTTP 501 NotImplemented, blocking all six EC2-agent CodeDeploy lifecycle checks before any CodeDeploy assertion runs

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInstanceProfile

## Description
Cause 1 of 1: every CodeDeploy check that needs a real EC2 deployment target first provisions the instance's IAM instance profile with iam:CreateInstanceProfile. That operation is still a scaffold stub and answers HTTP 501 `NotImplemented`, so all six checks abort in setup and none of CodeDeploy's on-instance lifecycle behavior is exercised at all.

AWS: iam CreateInstanceProfile creates an instance profile and returns it (InstanceProfile with Arn, InstanceProfileId, Path, CreateDate); it is the documented prerequisite for attaching a role to an EC2 instance that runs the CodeDeploy agent. Recorded behavior for the dependency itself: the checks call it as plain setup and expect a 200.
Emulator: HTTP 501, Code `NotImplemented`, Message "operation CreateInstanceProfile is scaffolded but not implemented in this emulator" (message quoted as an example of the stub channel). The handler at services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_instance_profile.rs:4 still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker.
Why: a 501 is a server fault, so a client cannot tell "not supported here" from "broken", and no EC2-backed CodeDeploy flow can be emulated: lifecycle event ordering, ApplicationStop-only-on-redeploy, hook failure propagation, fileExistsBehavior, minimumHealthyHosts concurrency bounds and auto-rollback artifact restoration are all unverified.
Fix: the operation's handler, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_instance_profile.rs, plus the instance-profile registry in the IAM service state (note: the fix lives in the IAM crate even though these failing checks are CodeDeploy checks — their check ids carry the `codedeploy` service token, which is what this issue is filed under). The already-filed issue "EMR control plane: the IAM instance-profile prerequisite and 7 EMR operation families answer 501 NotImplemented" names the same IAM stub from the EMR side; it does not carry these CodeDeploy replay keys.
Traces: codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact, codedeploy.ec2-agent-deployment-runs-appspec-hook-on-instance, codedeploy.file-exists-behavior-disallow-blocks-foreign-file-and-overwrite-replaces-it, codedeploy.hook-failure-skips-later-lifecycle-events-and-fails-the-target, codedeploy.lifecycle-event-order-and-applicationstop-only-on-redeploy, codedeploy.minimum-healthy-hosts-bounds-observed-concurrent-unavailability.

## Evidence
Replayed behavioral suite, codedeploy service. Six checks fail identically during setup:

botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateInstanceProfile operation: operation CreateInstanceProfile is scaffolded but not implemented in this emulator

raised from the harness client wrapper (context.py line 123 -> botocore client.py _make_api_call) for:
- codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact
- codedeploy.ec2-agent-deployment-runs-appspec-hook-on-instance
- codedeploy.file-exists-behavior-disallow-blocks-foreign-file-and-overwrite-replaces-it
- codedeploy.hook-failure-skips-later-lifecycle-events-and-fails-the-target
- codedeploy.lifecycle-event-order-and-applicationstop-only-on-redeploy
- codedeploy.minimum-healthy-hosts-bounds-observed-concurrent-unavailability

Expected: HTTP 200 with the created InstanceProfile.
Actual: HTTP 501, Code NotImplemented.
Source: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_instance_profile.rs:4 (`SCAFFOLDED, NOT IMPLEMENTED` marker).

## Steps to Reproduce
1. Boot the emulator.
2. iam.CreateRole with the EC2 trust policy, then iam.CreateInstanceProfile(InstanceProfileName="<name>") -> expect 200 with InstanceProfile.Arn; observe 501 NotImplemented.
3. Any CodeDeploy check that needs an EC2 deployment target (e.g. replay codedeploy.ec2-agent-deployment-runs-appspec-hook-on-instance) aborts at this call before reaching its CodeDeploy assertions.
