Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count
Issue-Key: cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route|run#50|AssertionError|assert len(igws) == 1, (
Issue-Key: cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group|_measure_estate#27|AssertionError|assert len(igws) == 1, (
Issue-Key: cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count|_wait_for_available_nats#43|Timeout|time.sleep(2.0)
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# cloudformation__ec2: 2 net-new root causes — a CDK-deployed VPC with a PUBLIC subnet group gets no attached internet gateway, and the max-azs clamp deploy never settles

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes, both reached through `cdk deploy` of an ec2.Vpc L2 construct.

Cause 1 of 2: a stack whose template declares an AWS::EC2::InternetGateway plus an AWS::EC2::VPCGatewayAttachment (what ec2.Vpc emits for any subnet_configuration containing a PUBLIC entry) reaches CREATE_COMPLETE, but no internet gateway is attached to the created VPC.
AWS: creating AWS::EC2::InternetGateway and AWS::EC2::VPCGatewayAttachment yields exactly one internet gateway attached to the VPC, so ec2 DescribeInternetGateways filtered by attachment.vpc-id=<vpc-id> returns exactly one gateway whose Attachments[].VpcId is that VPC; the public subnets' route table then carries 0.0.0.0/0 to it. Smithy (services/service-com-amazonaws-ec2-amazonec2): DescribeInternetGateways and AttachInternetGateway declare errors: [] (EC2 Query error codes are wire-level), so the requirement here is the resource's existence and attachment, not an error code.
Emulator: DescribeInternetGateways for the stack's VPC returns an empty list, while the stack is reported successful — the gateway resource is accepted and either never materialised or never attached.
Why: every client that derives subnet publicness, egress reachability or NAT placement from the attached gateway sees a VPC with no path to the internet while CloudFormation claims completion; CDK's Vpc.from_lookup classification and any egress audit built on DescribeInternetGateways is wrong.
Fix: the CloudFormation resource providers for AWS::EC2::InternetGateway and AWS::EC2::VPCGatewayAttachment, and the ec2 internet-gateway state they write, must mint the gateway and record the VPC attachment so the attachment.vpc-id filter resolves it.
Traces: cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route, cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group. Open issue 20261103T200742Z-ec2-a-stack-created-vpc-has-no-internet-gateway.md describes the same symptom under the ec2 heading for a different check; these two checks are not marked owned by it.

Cause 2 of 2: the max-azs clamp deployment never reaches a verdict — the check is cut off at the harness budget (60s) while deploying/polling, so no response is observed.
AWS: `ec2.Vpc(max_azs=N+5)` clamps to the N availability zones DescribeAvailabilityZones reports for the region and the stack settles: 2*N subnets, 2*N+1 route tables, and each private subnet's 0.0.0.0/0 route points at the single NAT gateway. Recorded behavior only for the settle time: the recording completes this deploy inside the check's budget.
Emulator: the check is cut off at the 60s budget with no verdict, so the clamp cannot be observed. Sibling checks in the same group do reach assertions, so what fails to settle is the clamp path's own work (the DescribeAvailabilityZones-driven subnet fan-out plus NAT-gateway convergence).
Why: a client deploying a multi-AZ VPC hangs instead of getting CREATE_COMPLETE, so nothing downstream of the VPC can be provisioned.
Fix: the CloudFormation stack-execution path for this template (subnet/route-table/NAT-gateway providers and the stack-status transitions they drive) must converge to a terminal stack status; a resource that cannot settle must fail the stack rather than stall.
Traces: cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count.

## Evidence
Cause 1 — expected: exactly one internet gateway attached to the stack's VPC. Actual (emulator): AssertionError: 0 internet gateways are attached to 'vpc-00000000000000026' ([]); the PUBLIC subnet_configuration entry makes ec2.Vpc create exactly one (check_..._cdk_l2_vpc_private_isolated_subnets_deploy_with_no_egress_route.py line 197), and AssertionError: 0 internet gateways are attached to 'vpc-00000000000000026'; ec2.Vpc creates exactly one (check_..._cdk_vpc_from_lookup_classifies_the_nat_egress_private_subnet_group.py line 278) — both after the stack reported success.
Cause 2 — expected: the stack settles and the AZ count clamps to the region's zone count (2*N subnets, 2*N+1 route tables, private 0.0.0.0/0 to the single NAT gateway). Actual (emulator): "Error: Check timed out after 60.0s" — no status, no error code, no verdict.

## Steps to Reproduce
Cause 1: cdk deploy an app whose ec2.Vpc subnet_configuration contains a PUBLIC entry (template emits AWS::EC2::InternetGateway + AWS::EC2::VPCGatewayAttachment). After CREATE_COMPLETE call ec2 DescribeInternetGateways(Filters=[{Name: 'attachment.vpc-id', Values: [<stack vpc id>]}]). Expected exactly one gateway attached to the VPC; observed an empty list.
Cause 2: cdk deploy an app with ec2.Vpc(max_azs = <region AZ count> + 5), poll the stack to a terminal status, then count subnets/route tables and check each private subnet's 0.0.0.0/0 target. Expected the stack settles and the AZ count clamps; observed the check is cut off at the 60s budget with no verdict.
