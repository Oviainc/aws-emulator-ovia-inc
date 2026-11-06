Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group cloudformation__ec2.cdk-l2-vpc-max-azs-two-deploys-two-nat-gateways cloudformation__ec2.cdk-l2-vpc-nat-gateways-one-overrides-the-per-az-default
Issue-Key: cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count|run#42|AssertionError|assert len(tables) == 2 * want_n + 1, (
Issue-Key: cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route|run#46|AssertionError|assert len(tables) == 2 * want_per_group + 1, (
Issue-Key: cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group|_measure_estate#27|AssertionError|assert len(hits) == 1, (
Issue-Key: cloudformation__ec2.cdk-l2-vpc-max-azs-two-deploys-two-nat-gateways|_wait_for_available_nats#26|Timeout|time.sleep(2.0)
Issue-Key: cloudformation__ec2.cdk-l2-vpc-nat-gateways-one-overrides-the-per-az-default|_wait_for_available_nats#27|Timeout|time.sleep(2.0)
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudformation__ec2: a CDK/CloudFormation-deployed VPC has no explicit route tables or subnet associations, and the NAT-gateway scenarios never settle (2 causes)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRouteTables

## Description
TWO net-new root causes in the cloudformation__ec2 surface. Both must be closed before submitting.

Cause 1 of 2 — AWS::EC2::RouteTable and AWS::EC2::SubnetRouteTableAssociation resources in a deployed stack create nothing in EC2: the VPC ends up with only its implicit main route table and no subnet is explicitly associated.
AWS: Smithy (EC2): DescribeRouteTables returns RouteTable{RouteTableId, VpcId, Routes, Associations}, and RouteTableAssociation carries {RouteTableAssociationId, RouteTableId, SubnetId, Main, AssociationState} with SubnetId documented as "not returned for an implicit association". So in AWS a CDK L2 VPC that declares one AWS::EC2::RouteTable plus one AWS::EC2::SubnetRouteTableAssociation per subnet yields, for that VPC, one route table per subnet PLUS the implicit main table the VPC always carries (2*N subnets -> 2*N+1 tables; 4 subnets -> 5 tables), and DescribeRouteTables filtered by association.subnet-id returns exactly one explicitly associated table per subnet.
Emulator: after a CREATE_COMPLETE deploy, DescribeRouteTables for the VPC returns exactly ONE table — the implicit main table — regardless of how many RouteTable resources the template declares ("the deployed VPC has 1 route tables (['rtb-...']); ... so 7 are required" and "... so 5 are required"), and each created subnet has 0 explicitly associated route tables ("subnet 'subnet-...' has 0 explicitly associated route tables ([])"). The rtb-/subnet- ids are per-run values; the counts and the absence of associations are the requirement.
Why: every VPC topology question a client asks is answered wrongly — public vs private vs isolated subnet classification (CDK's Vpc.from_lookup and every tool that reproduces it), egress-route checks, and NAT/IGW route verification all read route tables and their subnet associations. With a single main table, a subnet with no egress route is indistinguishable from a subnet that has no route table at all.
Fix: the CloudFormation resource providers for AWS::EC2::RouteTable, AWS::EC2::SubnetRouteTableAssociation and AWS::EC2::Route — they must create the route table in EC2 state, set the resource's physical id to the rtb- id, and record the subnet association (and its routes) so DescribeRouteTables reports them; and/or EC2 route-table state if the association is stored but not projected.
Traces: cloudformation__ec2.cdk-l2-vpc-max-azs-clamps-to-the-regions-availability-zone-count, cloudformation__ec2.cdk-l2-vpc-private-isolated-subnets-deploy-with-no-egress-route, cloudformation__ec2.cdk-vpc-from-lookup-classifies-the-nat-egress-private-subnet-group.

Cause 2 of 2 — the two CDK L2 VPC NAT-gateway scenarios never reach a verdict.
AWS: recorded behavior only. Both checks deploy a CDK L2 ec2.Vpc and then wait for its NAT gateway(s) to report state "available" before reading the private route tables; against AWS the gateways settle and the reads succeed.
Emulator: both checks are killed by the harness with "Check timed out after 60.0s" without reaching any assertion — nothing the scenario waits on (the deploy settling and/or NatGateway state becoming "available") is ever produced.
Why: the per-AZ-NAT default and the natGateways=1 override are entirely unverifiable; a client deploying a standard CDK VPC gets no observable NAT estate.
Fix: the CloudFormation/EC2 provisioning path for AWS::EC2::NatGateway (and the EIP it consumes) — the gateway must reach a terminal state DescribeNatGateways reports — plus cause 1's route-table projection the same scenarios read.
Traces: cloudformation__ec2.cdk-l2-vpc-max-azs-two-deploys-two-nat-gateways, cloudformation__ec2.cdk-l2-vpc-nat-gateways-one-overrides-the-per-az-default.

## Evidence
Cause 1: ec2.describe_route_tables(Filters=[{Name:"vpc-id",Values:[<vpc>]}]) after a CREATE_COMPLETE CDK VPC deploy -> expected 2*N+1 tables (7 for a 6-subnet VPC, 5 for a 4-subnet VPC); actual 1 table (only the implicit main table). ec2.describe_route_tables(Filters=[{Name:"association.subnet-id",Values:[<subnet>]}]) -> expected exactly 1 explicitly associated table per subnet; actual 0 ([]).
AssertionError: "the deployed VPC has 1 route tables (['rtb-...']); CDK gives each of the 6 subnets its own explicit route table, and the VPC additionally carries its own implicit main route table 'rtb-...', so 7 are required".
AssertionError: "the deployed VPC has 1 route tables (['rtb-...']); CDK gives each of the 4 subnets its own explicit table, plus the one implicit main route table every VPC carries, so 5 are required - having no egress route is not the same as having no route table".
AssertionError: "subnet 'subnet-...' has 0 explicitly associated route tables ([]); CDK associates exactly one per subnet, and an unassociated subnet silently inherits the VPC main table instead".
Cause 2: both NAT-gateway checks end with "Error: Check timed out after 60.0s" and no assertion output.

## Steps to Reproduce
1. cdk bootstrap, then cdk deploy an app whose stack is a single ec2.Vpc(max_azs=N) (or subnet_configuration with PRIVATE_ISOLATED subnets), so the synthesized template declares one AWS::EC2::RouteTable and one AWS::EC2::SubnetRouteTableAssociation per subnet. Wait for CREATE_COMPLETE.
2. ec2 DescribeRouteTables(Filters=[{Name:"vpc-id",Values:[<the VPC id>]}]) -> observe a single route table (the main one) instead of 2*N+1 (cause 1).
3. ec2 DescribeRouteTables(Filters=[{Name:"association.subnet-id",Values:[<any created subnet id>]}]) -> observe an empty result instead of exactly one explicitly associated table (cause 1).
4. Deploy the same app with the default natGateways (one per AZ) or natGateways=1 and poll ec2 DescribeNatGateways for State "available" -> observe the scenario never settles and the check is killed at 60s (cause 2).
