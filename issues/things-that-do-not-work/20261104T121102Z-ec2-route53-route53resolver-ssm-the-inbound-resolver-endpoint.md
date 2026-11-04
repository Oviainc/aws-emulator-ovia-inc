Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-query-denied-when-the-security-group-revokes-udp-53
Issue-Key: ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-query-denied-when-the-security-group-revokes-udp-53|_retry_dependency#8|Timeout|time.sleep(interval_s)
Issue-Commit: 3bd56f3581a93dde5b982de16e37f58616506edc

# ec2__route53__route53resolver__ssm: the inbound-resolver-endpoint DNS scenario never settles, so a security group that revokes UDP/53 is never observed denying the query (1 cause)

> _Note: this issue was identified against the code base at commit `3bd56f3581a93dde5b982de16e37f58616506edc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: the Route 53 Resolver inbound-endpoint data-plane scenario never reaches its observation point, because the EC2 instance it queries from never becomes a Systems Manager managed node.
AWS: the check builds a VPC (10.91.0.0/16) with two subnets, a private hosted zone carrying a record that resolves to 192.0.2.10, and an inbound Resolver endpoint; it launches an AL2023 t3.micro with an instance profile carrying arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, waits for the instance to register with Systems Manager, and then issues DNS queries from inside the guest through Run Command — first against the VPC resolver 169.254.169.253 and the endpoint addresses (answer: the record's address), then again after the endpoint's security group revokes UDP/53 (the query is denied). AWS registers such an instance shortly after boot, so DescribeInstanceInformation reports it with PingStatus Online (Smithy, services/service-com-amazonaws-ssm-amazonssm: InstanceInformation$PingStatus -> enum PingStatus Online | ConnectionLost | Inactive, with LastPingDateTime/AgentVersion/PlatformType/IamRole) and SendCommand/GetCommandInvocation then return the guest's stdout; the check budgets 300 s for the instance to run, 420 s for the agent and 600 s for the endpoint, i.e. it allows for AWS's own latency.
Emulator: the check never reaches an assertion — the suite kills it with `Error: Check timed out after 60.0s` and prints no response divergence, because no answer was ever returned to assert on. Hypothesis: the stall is the managed-node gate — nothing registers an EC2-launched instance as an SSM managed node, so the agent-online poll never converges and the in-guest DNS probes never run. The open issue 20261027T034339Z-ec2-ssm-a-launched-ec2-instance-never-becomes.md diagnoses that gate (services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs reports only nodes held in state's managed_nodes, which nothing populates for a launched instance) but does not carry this check id.
Why: a client cannot verify any Resolver endpoint behaviour from inside a VPC: it hangs instead of being answered or refused, with no error code to branch on and no service-side timeout. Both the positive path (the endpoint answers the private hosted zone) and the negative path (revoking UDP/53 on the endpoint's security group denies the query) are unobservable.
Fix: SSM managed-node registration in the ssm crate's state (populate the managed_nodes inventory for an instance whose profile can reach Systems Manager and project PingStatus Online on the emulator's owed-transition clock), then in-guest execution for SendCommand/GetCommandInvocation and the Resolver endpoint data path with security-group filtering on UDP/53.
Traces: ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-query-denied-when-the-security-group-revokes-udp-53.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; the check is listed under '### ec2__route53__route53resolver__ssm'.
Expected (AWS): after RunInstances of an AL2023 instance with an AmazonSSMManagedInstanceCore instance profile, DescribeInstanceInformation reports it with PingStatus 'Online' and Run Command returns the guest's dig/nslookup output — the private hosted zone record (192.0.2.10) via 169.254.169.253 and via the inbound endpoint's addresses, and a denied query once the endpoint security group's UDP/53 ingress is revoked.
Actual (emulator): verdict mismatch with detail 'Error: Check timed out after 60.0s' and no response diff — the check is killed inside its poll, before any assertion, so neither the answer nor the denial is ever observed.
Supporting: the already-open 20261027T034339Z-ec2-ssm-a-launched-ec2-instance-never-becomes.md records the same symptom ('Check timed out after 60.0s', no diff) for two ec2__ssm checks and locates the gate in services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs.

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Re-run the check id in trace_ids. 3. Manually: CreateVpc 10.91.0.0/16 with DNS support/hostnames, two subnets (10.91.1.0/24, 10.91.2.0/24); CreateHostedZone (private, associated with the VPC) with an A record -> 192.0.2.10; CreateResolverEndpoint (Direction=INBOUND) with a security group allowing UDP/53; create an IAM role/instance profile with arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore; RunInstances a t3.micro from /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 with that profile; poll ssm DescribeInstanceInformation for that InstanceId with PingStatus 'Online'. Expected: it appears Online, then SendCommand('AWS-RunShellScript') resolves the record through 169.254.169.253 and the endpoint IPs, and after RevokeSecurityGroupIngress of UDP/53 the query is denied. Actual: the instance never appears in InstanceInformationList, so the poll never converges and the scenario is killed at the harness budget.
