Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.instance-carries-both-security-groups-and-the-ssm-instance-profile ec2__ssm.send-command-to-an-instance-without-the-ssm-role-rejected-invalid-instance-id ec2__ssm.start-session-opens-and-terminates-a-channel-to-an-instance-with-no-ingress
Issue-Key: ec2__ssm.instance-carries-both-security-groups-and-the-ssm-instance-profile|run#16|AssertionError|assert inst.get("PublicIpAddress"), (
Issue-Key: ec2__ssm.send-command-to-an-instance-without-the-ssm-role-rejected-invalid-instance-id|_wait_online#11|Timeout|time.sleep(10)
Issue-Key: ec2__ssm.start-session-opens-and-terminates-a-channel-to-an-instance-with-no-ingress|_wait_online#12|Timeout|time.sleep(10)
Issue-Commit: c1cb63d17f24ad84276bf0784afa9b177e8d858c

# ec2__ssm: 2 net-new causes — a launch into a MapPublicIpOnLaunch subnet carries no PublicIpAddress, and a profile-bearing instance never becomes an SSM managed node (3 checks)

> _Note: this issue was identified against the code base at commit `c1cb63d17f24ad84276bf0784afa9b177e8d858c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Two independent root causes, both in the EC2/SSM launch-and-register path. Number of causes: 2.

Cause 1 of 2 — RunInstances into a subnet with MapPublicIpOnLaunch=true produces an instance with no public IPv4 address. Failing check: ec2__ssm.instance-carries-both-security-groups-and-the-ssm-instance-profile.
AWS: when an instance is launched into a subnet whose MapPublicIpOnLaunch attribute is true and no conflicting network-interface specification is given, EC2 auto-assigns a public IPv4 address to the primary network interface. DescribeInstances then reports it on the running instance as Instance.PublicIpAddress (Smithy, ec2#Instance member PublicIpAddress, wire name ipAddress: 'The public IPv4 address, or the Carrier IP address assigned to the instance, if applicable'), together with Instance.PublicDnsName and the association on Instance.NetworkInterfaces[].Association. The address itself is environment-specific; what is required is that a running instance in a public subnet has one at all.
Emulator: the instance reaches State.Name='running' in that subnet and DescribeInstances returns PublicIpAddress absent/None, so every derivation built on the launch (an SSH command, reachability analysis, anything that addresses the host from outside the VPC) has nothing to use. Replay detail: 'AssertionError: the subnet has MapPublicIpOnLaunch true, so the instance must carry a public IP ... DescribeInstances reports None'.
Why: a client that launches into a public subnet and then connects to, or publishes, the instance's public address gets a null where AWS gives an address — the launch looks successful but is unusable, and code that formats a connection string emits 'None'.
Fix: the ec2 RunInstances handler and instance state — honour the target subnet's MapPublicIpOnLaunch when allocating the primary ENI, record the assigned public address (and its ENI association), and serialize it in the DescribeInstances Instance representation.

Cause 2 of 2 — an instance launched with an AmazonSSMManagedInstanceCore instance profile never becomes a Systems Manager managed node, so every check that gates on registration exhausts the replay budget with no verdict. Failing checks: ec2__ssm.send-command-to-an-instance-without-the-ssm-role-rejected-invalid-instance-id and ec2__ssm.start-session-opens-and-terminates-a-channel-to-an-instance-with-no-ingress.
AWS: an EC2 instance whose instance profile carries AmazonSSMManagedInstanceCore registers with Systems Manager shortly after boot; DescribeInstanceInformation then returns an entry for that instance id with PingStatus='Online' (Smithy, ssm: DescribeInstanceInformation -> InstanceInformationList). Registration is the precondition both checks then measure against: SendCommand against an UNREGISTERED instance is refused InvalidInstanceId (Smithy, ssm: SendCommand errors include InvalidInstanceId) while the registered twin runs the document, and StartSession against a registered instance returns SessionId/TokenValue/StreamUrl (Smithy, ssm: StartSession output; TargetNotConnected is the refusal when the target is not a connected managed node).
Emulator: both checks block waiting for the just-launched, profile-bearing instance to appear with PingStatus='Online' in DescribeInstanceInformation and are cut off by the harness cap — replay detail for both: 'Check timed out after 180.0s'. Hypothesis: the hang is the registration barrier each check performs immediately after RunInstances (the instance never enters the SSM inventory), since no other call in either flow blocks; no response of any kind is produced, so neither the InvalidInstanceId refusal nor the StartSession success is exercised.
Why: a client that launches a managed node and then waits for it to come Online waits forever, and the two refusals that depend on registration state (InvalidInstanceId for an unmanaged target, TargetNotConnected for a disconnected one) can never be told apart from 'not yet registered'. Every Run Command and Session Manager workflow built on a freshly launched instance is unreachable.
Fix: the ssm managed-node registration/state component together with the ec2 launch path that feeds it — a launched instance with an SSM-core instance profile must become an inventory entry with PingStatus Online (and be removed when it stops/terminates), so DescribeInstanceInformation, SendCommand and StartSession resolve it. Open issue 20261027T034339Z-ec2-ssm-a-launched-ec2-instance-never-becomes.md describes the same registration gap for other checks and does not carry these two checks' replay keys; the CloudFormation-flavoured variants 20261107T112846Z / 20261107T133755Z / 20261107T201046Z-cloudformation-ec2-ssm-* are a different service heading.

Traces: ec2__ssm.instance-carries-both-security-groups-and-the-ssm-instance-profile (cause 1); ec2__ssm.send-command-to-an-instance-without-the-ssm-role-rejected-invalid-instance-id and ec2__ssm.start-session-opens-and-terminates-a-channel-to-an-instance-with-no-ingress (cause 2).

## Evidence
Cause 1 — expected (AWS): RunInstances into a subnet with MapPublicIpOnLaunch=true, instance running => DescribeInstances Reservations[].Instances[].PublicIpAddress is a non-empty IPv4 string (member PublicIpAddress / wire ipAddress). Actual (emulator): the field is absent/None while State.Name='running' and SecurityGroups/IamInstanceProfile are populated. Replay: 'AssertionError: the subnet has MapPublicIpOnLaunch true, so the instance must carry a public IP — that is what makes the exported SshCommand look usable; DescribeInstances reports None'.

Cause 2 — expected (AWS): within a couple of minutes of launch, DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:[<iid>]}]) returns one entry with PingStatus='Online' for the profile-bearing instance (and [] for an instance with no profile); SendCommand on the unmanaged instance is then refused InvalidInstanceId and creates no invocation, while StartSession on the managed one returns a wss:// StreamUrl, a non-empty TokenValue and a SessionId. Actual (emulator): no registration ever appears; both checks produce no response at all and are cut off — replay detail for each: 'Check timed out after 180.0s'.

## Steps to Reproduce
Cause 1:
1. Pick the default VPC and a subnet with MapPublicIpOnLaunch=true; create two security groups (one with no ingress, one allowing tcp/22 from the VPC CIDR).
2. Create a role with AmazonSSMManagedInstanceCore and an instance profile for it.
3. RunInstances (t3.micro) into that subnet with both groups and the instance profile; wait for State.Name='running'.
4. DescribeInstances on the instance id: AWS reports a PublicIpAddress; the emulator reports none.

Cause 2:
1. Same VPC/subnet/profile setup; RunInstances one instance WITH the SSM-core instance profile and (for the first check) one WITHOUT any profile.
2. Poll DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:[<profile-bearing iid>]}]) for PingStatus='Online'. AWS reports Online; the emulator returns an empty list indefinitely and the check is killed at the 180s harness cap.
3. Consequently never reached: SendCommand(InstanceIds=[<no-profile iid>], DocumentName='AWS-RunShellScript') must be refused InvalidInstanceId with no invocation created, and StartSession(Target=<profile-bearing iid>) must return SessionId/TokenValue/wss:// StreamUrl, appear under DescribeSessions(State='Active') and move to History with an EndDate after TerminateSession.
