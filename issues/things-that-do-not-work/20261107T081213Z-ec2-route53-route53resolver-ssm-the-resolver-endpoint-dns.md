Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-addresses-answer-the-private-hosted-zone ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-query-denied-when-the-security-group-revokes-udp-53
Issue-Key: ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-addresses-answer-the-private-hosted-zone|_poll#8|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-query-denied-when-the-security-group-revokes-udp-53|_poll#8|Timeout|time.sleep(interval_s)
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# ec2__route53__route53resolver__ssm: the resolver-endpoint DNS scenarios never settle — a launched instance never becomes an Online SSM managed node, so no DNS query is ever made from the guest

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
One net-new root cause grounded in failing checks the suite lists under '### ec2__route53__route53resolver__ssm'. 1 CAUSE.

Cause 1 of 1: the EC2 instance these scenarios launch as their DNS client never registers as a Systems Manager managed node, so the in-guest `dig`/resolver query that is the scenarios' only observation point never runs and both checks are killed while waiting for the agent.
AWS: an instance launched with an instance profile carrying arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore registers with Systems Manager shortly after boot and DescribeInstanceInformation reports it with PingStatus 'Online' (Smithy, services/service-com-amazonaws-ssm-amazonssm: InstanceInformation$PingStatus -> enum PingStatus Online | ConnectionLost | Inactive, plus LastPingDateTime, AgentVersion, IamRole); SendCommand/GetCommandInvocation then run in the guest and return its stdout, which is how each check resolves a private-hosted-zone record through the inbound Route 53 Resolver endpoint's IP addresses and how it observes that the query stops being answered once the endpoint's security group revokes UDP/53.
Emulator: neither check reaches an assertion. The first dies in its agent wait with 'TimeoutError: the SSM agent on i-00000000000000001 coming Online: the run budget ended first; not graded' raised from `_poll(_agent, lambda i: i is not None and i.get("PingStatus") == "Online", ...)`; the second is killed with 'Error: Check timed out after 60.0s' with no response diff, in the same wait. Hypothesis: nothing registers an EC2-launched instance as an SSM managed node, so DescribeInstanceInformation never reports it.
Why: the failure mode is a hang with no error code to branch on, and the entire hybrid-DNS story (private hosted zone answered through an inbound resolver endpoint, and the security group that gates UDP/53 to it) is unobservable. The open issue 20261104T121102Z-ec2-route53-route53resolver-ssm-the-inbound-resolver-endpoint.md records this scenario never settling but does not carry these check ids.
Fix: SSM managed-node registration in the ssm crate's state plus in-guest execution for SendCommand/GetCommandInvocation, so the launched DNS client is projected Online and its queries return output; the resolver-endpoint/security-group evaluation the scenarios then assert on sits in the route53resolver and ec2 crates.
Traces: ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-addresses-answer-the-private-hosted-zone, ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-query-denied-when-the-security-group-revokes-udp-53.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; both checks are listed under '### ec2__route53__route53resolver__ssm'.

ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-addresses-answer-the-private-hosted-zone: 'File check_..._inbound_resolver_endpoint_addresses_answer_the_private_hosted_zone.py, line 339, in _await_agent -> agent = _poll(_agent, lambda i: i is not None and i.get("PingStatus") == "Online", ...) -> TimeoutError: the SSM agent on i-00000000000000001 coming Online: the run budget ended first; not graded'.
ec2__route53__route53resolver__ssm.inbound-resolver-endpoint-query-denied-when-the-security-group-revokes-udp-53: 'Error: Check timed out after 60.0s', no response diff printed; the body performs the same agent-online wait before its first assertion.
Expected (AWS): DescribeInstanceInformation lists the launched instance with PingStatus 'Online', after which SendCommand returns the guest's DNS answer. Actual (emulator): the instance never appears and the poll runs to the caller's deadline. (The instance id is this run's value.)

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run the check ids in trace_ids.
3. Create a VPC/subnets, a private hosted zone with a record, a Route 53 Resolver inbound endpoint with a security group allowing UDP/53, and an EC2 instance with an AmazonSSMManagedInstanceCore instance profile; poll ssm DescribeInstanceInformation(Filters=[{'Key':'InstanceIds','Values':[<id>]}]) for PingStatus 'Online' and then SendCommand('AWS-RunShellScript') with a dig against the endpoint's IP addresses. Expected: Online within registration latency and the record's value in the command output; after revoking UDP/53 the query is no longer answered. Actual: the instance never becomes Online and both checks are killed in that wait.
