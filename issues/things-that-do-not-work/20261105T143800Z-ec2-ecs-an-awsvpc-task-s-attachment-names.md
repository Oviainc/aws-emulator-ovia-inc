Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs.awsvpc-task-eni-matches-the-ec2-view-and-is-released-on-stop
Issue-Key: ec2__ecs.awsvpc-task-eni-matches-the-ec2-view-and-is-released-on-stop|run#5|AssertionError|assert eni is not None, (
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ec2__ecs: an awsvpc task's attachment names a synthetic networkInterfaceId (eni-00000000000000001) that EC2 DescribeNetworkInterfaces does not know

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: the ENI id ECS publishes in a RUNNING awsvpc task's attachment does not exist in EC2.
AWS: for an awsvpc task, DescribeTasks returns an attachment of type ElasticNetworkInterface whose details carry networkInterfaceId, macAddress, subnetId and privateIPv4Address (Smithy: ecs#Attachment.details — "For elastic network interfaces, this includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address"). That networkInterfaceId names a real EC2 network interface: ec2 DescribeNetworkInterfaces with that id returns it, in the task's subnet, with the same private IPv4 and MAC, and the interface is released (and then not found) after the task reaches STOPPED. The ENI id itself is a per-run generated value, not a requirement; what is required is that the id ECS reports resolves in EC2.
Emulator: ECS reports networkInterfaceId 'eni-00000000000000001' for the RUNNING task, and EC2 DescribeNetworkInterfaces has no such interface — the attachment names an ENI that does not exist. The value is a fixed placeholder-shaped id rather than an interface the EC2 side actually created, so the ECS and EC2 views of the same task are unjoinable.
Why: every client that cross-references the two services breaks: security-group/subnet audits, ENI-based tooling, `aws ec2 describe-network-interfaces --network-interface-ids <from DescribeTasks>` (which answers InvalidNetworkInterfaceID.NotFound instead of the interface), and any ENI-release check after StopTask. It also makes the task's real private IP undiscoverable, which is what ALB `ip` target registration depends on.
Fix: the ECS task-launch path for awsvpc tasks must create the ENI in EC2 state (or bind to an existing one) and publish that interface's real id/MAC/private IPv4 in the attachment details, and release it on task STOP.
Traces: ec2__ecs.awsvpc-task-eni-matches-the-ec2-view-and-is-released-on-stop.

## Evidence
Expected (AWS): the networkInterfaceId in the task's ElasticNetworkInterface attachment resolves via ec2 DescribeNetworkInterfaces, in the task's subnet, with a matching privateIpAddress and MAC, and is gone after the task reaches STOPPED.
Actual (emulator): AssertionError: "ECS reports networkInterfaceId 'eni-00000000000000001' for the RUNNING task, but EC2 has no such interface — the attachment names an ENI that does not exist".

## Steps to Reproduce
1. Create a VPC (10.84.0.0/16), a subnet (10.84.1.0/24) and a security group.
2. RegisterTaskDefinition with networkMode=awsvpc, requiresCompatibilities=[FARGATE].
3. RunTask into that subnet and wait for lastStatus=RUNNING.
4. Read attachments[].details networkInterfaceId from DescribeTasks and pass it to ec2 DescribeNetworkInterfaces → the interface does not exist.
