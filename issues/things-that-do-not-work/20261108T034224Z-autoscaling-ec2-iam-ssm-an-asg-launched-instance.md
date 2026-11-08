Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected
Issue-Key: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected|_poll#13|Timeout|last = probe()
Issue-Commit: a278882e3688d81a20fde96bfe4d3c7b3cce506e

# autoscaling__ec2__iam__ssm: an ASG-launched instance with AmazonSSMManagedInstanceCore never becomes an Online SSM managed node, so the StartSession permission-chain never settles (1 cause)

> _Note: this issue was identified against the code base at commit `a278882e3688d81a20fde96bfe4d3c7b3cce506e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: an EC2 instance launched by an Auto Scaling group from a launch template carrying an instance profile whose role has AmazonSSMManagedInstanceCore is never registered as an SSM managed node, so ssm DescribeInstanceInformation never reports it (never PingStatus=Online) and the check's positive barrier is never reached; the whole scenario therefore exhausts the replay budget ('Check timed out after 180.0s') and the negative half — StartSession against the sibling instance whose role lacks that policy — is never evaluated. Failing check id: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected.

NOTE on prior art: the open issue 20261104T042502Z-autoscaling-ec2-iam-ssm-an-asg-instance-never.md names this same check id and the same missing registration, but the replay report did NOT mark this failure as OWNED by it or as ALREADY FILED (that issue is keyed to a different failing step/budget: _poll#13 at a 60.0s budget, where this run times out at 180.0s). It is filed here so the failure is not lost; a fixer may close both together, since the component to repair is the same. Related but NOT covering: 20261022T175547Z-autoscaling-auto-scaling-4-fidelity-divergences-cloudformation-asg-never.md (CloudFormation-declared ASGs never provisioned, keyed to the sibling e2e check), and the EC2/SSM managed-node issues keyed to plain RunInstances launches (e.g. 20261107T081213Z-ec2-iam-s3-ssm-an-instance-launched-with.md, 20261108T004524Z-ec2-ssm-an-instance-whose-profile-role-gains.md); this one is reached through an Auto Scaling group's launch path.

AWS: an instance the Auto Scaling group brings InService, launched from a launch template whose IamInstanceProfile names a profile whose role has arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore and whose subnet routes to an internet gateway, registers with Systems Manager: ssm DescribeInstanceInformation (Smithy: DescribeInstanceInformation, output InstanceInformationList, paginated by NextToken, errors InternalServerError/InvalidFilterKey/InvalidInstanceId/InvalidInstanceInformationFilterValue/InvalidNextToken) returns an InstanceInformation entry for that InstanceId with PingStatus 'Online', and ssm StartSession(Target=<that id>) (Smithy: StartSession, output SessionId/TokenValue/StreamUrl, errors InternalServerError/InvalidDocument/TargetNotConnected) returns a SessionId and TokenValue that TerminateSession then closes. An otherwise identical instance whose role lacks that policy never appears in DescribeInstanceInformation at all, and StartSession against it is refused with error code TargetNotConnected, HTTP 400, message '<instance-id> is not connected.' — fixed text ' is not connected.' with the requested Target instance id as its only variable part. Registration timing is recorded behavior only; the recording reaches the barrier well inside the check's own 480s deadlines.

Emulator: the check never reaches a verdict — it ends 'Error: Check timed out after 180.0s' while polling DescribeInstanceInformation for the SSM-core instance to report PingStatus 'Online'. No wrong answer is produced for StartSession; the managed-node registration simply never happens, so the poll cannot terminate and neither the positive session nor the TargetNotConnected refusal is ever observed.

Why: a client that provisions SSM-managed capacity through an Auto Scaling group and waits for the instance to come Online before opening a session waits forever — it gets neither a managed node nor an actionable refusal. Worse, the IAM distinction the scenario exists to prove is unobservable: because no instance ever registers, 'absent from SSM' cannot be distinguished from 'not yet registered', so a least-privilege test that should FAIL when AmazonSSMManagedInstanceCore is missing can never pass or fail meaningfully against the emulator.

Fix: the reconciliation that turns a launched EC2 instance (including one launched by the Auto Scaling group's own launch path, not only a direct RunInstances) carrying an instance profile whose role grants the SSM core actions into an SSM managed node — the SSM instance-information state and whatever feeds it from the EC2/autoscaling launch path — not StartSession's own validation, which only needs to answer TargetNotConnected for an instance that is absent from that state.

Traces: autoscaling__ec2__iam__ssm.asgssh-ssm-start-session-to-instance-without-ssm-core-policy-rejected (the sibling e2e check in the same group, asgssh-e2e-troubleshoot-asg-ssh-connectivity-lifecycle, is OWNED by an existing issue and is not part of this one).

## Evidence
Expected (AWS): after both ASGs report one InService instance, ssm.describe_instance_information(Filters=[{'Key':'InstanceIds','Values':[control, bare]}]) returns an entry for the control instance with PingStatus='Online' within the check's 480s registration deadline; ssm.start_session(Target=control) returns a SessionId and a TokenValue and ssm.terminate_session(SessionId=...) echoes that SessionId; for 30s after that barrier the bare instance is absent from every DescribeInstanceInformation page, and ssm.start_session(Target=bare) raises ClientError with Code 'TargetNotConnected' and Message '<bare-instance-id> is not connected.' (HTTP 400).
Actual (emulator): the check ends 'Error: Check timed out after 180.0s' inside the poll for 'the control instance <id> Online in SSM' — DescribeInstanceInformation never lists the SSM-core instance (no entry, hence no PingStatus at all), so StartSession is never called and no refusal is produced. The 180s figure is this run's harness budget, not the requirement; the divergence is that registration never converges.

## Steps to Reproduce
Boot the emulator and point botocore clients for 'ec2', 'autoscaling', 'iam' and 'ssm' at it, then:
1. Create a VPC with an internet gateway, a route table sending 0.0.0.0/0 to it, one subnet in an AZ that offers t3.micro, and a security group with no ingress.
2. Create two IAM roles trusting ec2.amazonaws.com, attaching arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore to the first only; put each in its own instance profile.
3. Create one launch template per profile (t3.micro, newest Amazon-owned AL2023 x86_64 AMI, IMDSv2 required, one interface in the subnet with AssociatePublicIpAddress) and one Auto Scaling group per template; scale each to MinSize=DesiredCapacity=1 and wait for one InService instance each.
4. Poll ssm.describe_instance_information(Filters=[{'Key':'InstanceIds','Values':[both ids]}]) for the SSM-core instance to appear with PingStatus='Online' — observe that it never appears, so the poll never terminates and ssm.start_session is never reached.
5. (The behavior under test, once registration works: start_session/terminate_session succeed for the SSM-core instance, while start_session on the other is refused TargetNotConnected '<id> is not connected.')
Equivalently, re-replay the behavioral check listed in trace_ids.
