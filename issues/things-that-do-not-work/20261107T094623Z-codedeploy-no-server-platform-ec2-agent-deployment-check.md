Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.agent-hands-the-hook-the-real-deployment-identifiers codedeploy.applicationstop-hook-failure-blocks-redeploy-until-ignored codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact codedeploy.deployment-group-alarm-in-alarm-state-blocks-the-deployment codedeploy.ec2-agent-deployment-runs-appspec-hook-on-instance codedeploy.file-exists-behavior-disallow-blocks-foreign-file-and-overwrite-replaces-it codedeploy.hook-failure-skips-later-lifecycle-events-and-fails-the-target codedeploy.lifecycle-event-order-and-applicationstop-only-on-redeploy
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.agent-hands-the-hook-the-real-deployment-identifiers|_wait_ssm_online#12|Timeout|time.sleep(5)
Issue-Key: codedeploy.applicationstop-hook-failure-blocks-redeploy-until-ignored|_wait_ssm_online#16|Timeout|time.sleep(5)
Issue-Key: codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact|_wait_fleet_ready#15|Timeout|time.sleep(5)
Issue-Key: codedeploy.deployment-group-alarm-in-alarm-state-blocks-the-deployment|_wait_ssm_online#16|TimeoutError|raise TimeoutError(f"instance {instance_id!r} Online in SSM: the run budget ended first; not graded")
Issue-Key: codedeploy.ec2-agent-deployment-runs-appspec-hook-on-instance|_wait_ssm_online#15|TimeoutError|raise TimeoutError(f"instance {instance_id!r} Online in SSM: the run budget ended first; not graded")
Issue-Key: codedeploy.file-exists-behavior-disallow-blocks-foreign-file-and-overwrite-replaces-it|_wait_fleet_ready#18|TimeoutError|raise TimeoutError(f"{sorted(pending)!r} Online in SSM: the run budget ended first; not graded")
Issue-Key: codedeploy.hook-failure-skips-later-lifecycle-events-and-fails-the-target|_wait_fleet_ready#15|Timeout|time.sleep(5)
Issue-Key: codedeploy.lifecycle-event-order-and-applicationstop-only-on-redeploy|_wait_fleet_ready#12|Timeout|time.sleep(5)
Issue-Commit: 6687e38a12cdc9df434fb5352a33398cb021dd22

# codedeploy: no Server-platform (EC2 agent) deployment check can start — a launched EC2 instance never becomes an Online SSM managed node (8 checks)

> _Note: this issue was identified against the code base at commit `6687e38a12cdc9df434fb5352a33398cb021dd22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: every behavioral check that exercises CodeDeploy's Server compute platform against a real EC2 host is blocked before the deployment is even created, because the instance the check launches never appears as an Online managed node in Systems Manager. The checks launch an AL2023 instance (AMI id read from the public SSM parameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64), attach an instance profile and user-data that installs SSM Agent and the CodeDeploy agent, and then poll ssm:DescribeInstanceInformation with Filters=[{Key: InstanceIds, Values: [<instance-id>]}] until PingStatus == "Online" before registering the host as a deployment target. That poll never succeeds, so the Server-platform surface (lifecycle event ordering, ApplicationStop semantics, hook failure propagation, fileExistsBehavior, alarm gating, auto-rollback artifact reinstall, and the hook environment variables handed to the agent) is entirely unobservable.

AWS: a running EC2 instance with SSM Agent and an instance profile granting AmazonSSMManagedInstanceCore registers itself with Systems Manager within a minute or two, after which DescribeInstanceInformation returns one InstanceInformation entry for it. Smithy (ssm): InstanceInformation members InstanceId (pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$), PingStatus (enum Online | ConnectionLost | Inactive, doc "Connection status of SSM Agent"; Inactive is deprecated and no longer used), ResourceType (EC2 instance vs managed instance), PlatformType/PlatformName/PlatformVersion, AgentVersion (pattern ^[0-9]{1,6}(\.[0-9]{1,6}){2,3}$), LastPingDateTime, IPAddress, ComputerName. Once the node is Online, ssm:SendCommand/GetCommandInvocation can run in-guest commands on it (that is how these checks read the deployment's on-host effect), and the CodeDeploy agent on the host polls for deployment commands so a Server-platform deployment can progress through its lifecycle events. Hypothesis: the emulator models RunInstances state only in EC2 and never cross-registers the launched instance into SSM's managed-node inventory, so no PingStatus is ever reported.

Emulator: DescribeInstanceInformation keeps returning an empty InstanceInformationList for the launched instance id (the launches succeed — the checks hold a concrete id, e.g. a single-instance id of the form i-0000000000000000N). The three checks whose own wait budget (420s/600s) elapsed inside the run budget report the check's own diagnostic verbatim: TimeoutError: instance 'i-…' Online in SSM: the run budget ended first; not graded, and TimeoutError: ['i-…'] Online in SSM: the run budget ended first; not graded. The remaining five are cut off by the 60s harness cap inside the same poll loop ("Check timed out after 60.0s"). No deployment is ever created in any of the eight, so nothing downstream of the host registration is graded.

Why: a client cannot use this emulator for any EC2/on-instance CodeDeploy workflow. The entire Server compute platform — AppSpec file installs, hooks, hook environment (DEPLOYMENT_ID/APPLICATION_NAME/DEPLOYMENT_GROUP_NAME/LIFECYCLE_EVENT), ApplicationStop-on-redeploy, fileExistsBehavior, instance-target lifecycle statuses, alarm-gated stops and auto-rollback reinstalls — is unreachable, and because the blocking symptom is a silent empty list rather than an error, a client's own poll loops hang instead of failing fast.

Fix: the EC2 instance lifecycle must cross-register a running instance as an SSM managed node, so the SSM managed-node state (the backing store behind DescribeInstanceInformation) carries an entry with PingStatus "Online" shortly after the instance reaches running, and the in-guest command plane (SendCommand/GetCommandInvocation) and the CodeDeploy agent polling plane can then act on that host. The CodeDeploy side belongs in the Server-platform deployment execution path (the instance-target lifecycle driver) once a host exists to drive. Note this is the codedeploy-side instance of a symptom already filed for other service groups — see open issues 20261107T081213Z-ssm-3-net-new-causes-a-launched-ec2.md, 20261107T081213Z-ec2-iam-s3-ssm-an-instance-launched-with.md and 20261107T075601Z-ec2-ssm-windows-amis-report-platform-windows-instead.md; none of them carries these codedeploy checks' replay keys.

Traces: the eight codedeploy checks listed in trace_ids; all eight stop in _wait_ssm_online / _wait_fleet_ready before CreateDeployment.

## Evidence
Expected (AWS): after RunInstances of an AL2023 instance with an SSM-capable instance profile, ssm:DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:['i-…']}]) returns InstanceInformationList with one entry whose PingStatus is 'Online' (enum Online|ConnectionLost|Inactive), after which SendCommand can run `systemctl is-active codedeploy-agent` on the host and a Server-platform CreateDeployment can drive that instance target through its lifecycle events.

Actual (emulator): InstanceInformationList stays empty for the whole wait. Verbatim check failures:
- codedeploy.deployment-group-alarm-in-alarm-state-blocks-the-deployment: line 63 _wait_ssm_online(ctx, ssm, instance_id) -> line 712 raise TimeoutError -> "TimeoutError: instance 'i-00000000000000001' Online in SSM: the run budget ended first; not graded"
- codedeploy.ec2-agent-deployment-runs-appspec-hook-on-instance: line 74 _wait_ssm_online(...) -> line 580 -> "TimeoutError: instance 'i-00000000000000001' Online in SSM: the run budget ended first; not graded"
- codedeploy.file-exists-behavior-disallow-blocks-foreign-file-and-overwrite-replaces-it: line 82 _wait_fleet_ready(ctx, ssm, [instance_id]) -> line 776 -> "TimeoutError: ['i-00000000000000001'] Online in SSM: the run budget ended first; not graded"
- codedeploy.agent-hands-the-hook-the-real-deployment-identifiers, codedeploy.applicationstop-hook-failure-blocks-redeploy-until-ignored, codedeploy.auto-rollback-on-deployment-failure-reinstalls-the-prior-artifact, codedeploy.hook-failure-skips-later-lifecycle-events-and-fails-the-target, codedeploy.lifecycle-event-order-and-applicationstop-only-on-redeploy: "Check timed out after 60.0s", each inside the same _wait_ssm_online/_wait_fleet_ready poll (sources at lines 534, 1368, 2803, 11261, 14188 of the suite report).

The instance ids in these messages are per-run values, not part of the requirement; the requirement is that a running instance becomes an Online managed node at all.

## Steps to Reproduce
1. Create a VPC/subnet, an instance profile with SSM + CodeDeploy permissions, and read the AL2023 AMI id from the public SSM parameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64.
2. ec2:RunInstances one t3.micro from that AMI with the instance profile and user-data installing SSM Agent and the CodeDeploy agent; wait for state 'running'.
3. Poll ssm:DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:[<instance-id>]}]).
4. Observe InstanceInformationList is empty indefinitely — no entry, hence no PingStatus 'Online' — so no Server-platform codedeploy:CreateDeployment against that tagged instance can be attempted.
