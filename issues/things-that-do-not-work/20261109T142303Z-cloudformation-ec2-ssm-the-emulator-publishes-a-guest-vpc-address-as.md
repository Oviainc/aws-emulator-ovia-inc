Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance
Issue-Key: cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance|run#16|PublishedUrlRejected|target_ip = ctx.endpoint_host(
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# The emulator publishes a guest VPC address as an endpoint host, so a CFN-deployed EC2 security-group scenario is handed an address it does not serve

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Cause 1 of 1: the emulator announces a private VPC address (the instance's in-VPC private IP, port 8080) as the host a client should reach, instead of one of its own reachable hosts.

AWS: an instance's PrivateIpAddress is reachable from inside the VPC, and a client driving a guest-to-guest security-group test reaches the peer at that address from another instance in the same VPC — the address the API hands out is the address that is actually served on the path the API tells the client to use. Smithy (ec2): RunInstances output Instances[].PrivateIpAddress / NetworkInterfaces[].PrivateIpAddress. Recorded behavior only for the endpoint-publication rule: an emulator must announce an address on which it is itself reachable, because the client has no other path to the guest.

Emulator: the address published for the probe target is `10.43.1.4:8080` — a VPC-internal address that is not one of the emulator's own hosts and that the client cannot reach. The publication is refused at the boundary (`PublishedUrlRejected: endpoint_host was given 10.43.1.4:8080 (published by the emulator or built by the check): its host is not one of the emulator's own hosts`), so the security-group scenario (group rename replaces the group; a source-group rule still drops an unrelated instance) is never exercised.

Why: a client that follows the address the API gives it cannot reach the workload at all. Every guest-to-guest reachability conclusion — a source-group rule admitting members only, a renamed group replacing the old one, a drop presenting as a timeout rather than a refusal — becomes unobservable, and a client cannot distinguish "the security group dropped it" from "the address was never served".

Fix: the endpoint/address publication path for EC2 instances and their in-guest services — the emulator must announce a host it serves (its own endpoint host, a documented own host, or a loopback address for a loopback endpoint) when it hands a client an address to connect to, and map it to the guest's VPC address internally.

Traces: check cloudformation__ec2__ssm.deploy-ec2-3-group-rename-replaces-the-group-and-source-group-rule-still-drops-an-unrelated-instance.

Related but not covering: open issue 20261108T120914Z-cloudformation-ec2-ssm-cfn-deployed-ec2-ssm-estates.md mentions the emulator handing out guest IPs it does not serve, but this check is not listed there and fails on its own published-address refusal.

## Evidence
Expected: the address published for the probe target is a host the emulator serves, so the in-guest probe can connect (or time out because the security group drops it, which is the behaviour under test).

Actual: `simparity.published.PublishedUrlRejected: endpoint_host was given 10.43.1.4:8080 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address`.
The diverging element is the class of address announced — a VPC-private 10.x address rather than an own host; the specific octets and the port are per-run values and not part of the requirement.

## Steps to Reproduce
1. CreateStack an EC2 estate with two instances in one VPC, a source security group and a group whose ingress rule references the source group on port 8080.
2. Ask the emulator for the address at which the target instance's port 8080 is reachable (the address it publishes for the guest endpoint).
3. The published host is the instance's VPC-private address (e.g. 10.x.x.x:8080), which is not one of the emulator's own served hosts, so the connection path the API advertises does not exist.
