Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.create-db-subnet-group-single-az-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.create-db-subnet-group-single-az-rejected|run#20|AssertionError|assert len(created_azs) >= 2, (
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# rds: a DB subnet group's reported Availability Zones are hashed from the subnet id instead of resolved from EC2, so a two-AZ group reads back as covering one AZ

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
One net-new root cause in the rds service, grounded in a failing behavioral check whose replay key no filed issue carries.

Cause 1 of 1: the Availability Zone a DB subnet group reports for each of its subnets is synthesized from the subnet id, not resolved from the EC2 subnet record, so a group built from subnets that lie in two different AZs reads back as covering a single AZ (and, symmetrically, the AZ-coverage precondition is decided on the same synthesized value).

AWS: CreateDBSubnetGroup resolves every entry of SubnetIds against EC2 and the group it stores carries, for each subnet, that subnet's ACTUAL Availability Zone. Smithy (com.amazonaws.rds#AmazonRDSv19): DBSubnetGroup$Subnets is a SubnetList of Subnet{SubnetIdentifier, SubnetAvailabilityZone -> AvailabilityZone{Name}, SubnetOutpost, SubnetStatus}; DBSubnetGroup is the element DescribeDBSubnetGroups reports and the output member of CreateDBSubnetGroup; CreateDBSubnetGroup documents "DB subnet groups must contain at least one subnet in at least two AZs in the Amazon Web Services Region" and models DBSubnetGroupDoesNotCoverEnoughAZs (errorKind client, httpError 400) for a list that does not. The requirement is therefore: for every subnet id in SubnetIds, Subnet$SubnetAvailabilityZone$Name equals the AvailabilityZone that EC2 DescribeSubnets reports for that subnet id, and the set of those names is the group's AZ coverage. The AZ names themselves are per-run values of the request's own network, not part of the contract.

Emulator: the zone is derived from the subnet id by hash. services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs defines `subnet_availability_zone(region, subnet_id)` as `region + AVAILABILITY_ZONE_SUFFIXES[sha256(subnet_id)[0] % 3]` (state.rs:2838-2846); `DbSubnetGroupRecord::to_wire` fills every Subnet$SubnetAvailabilityZone$Name from it (state.rs:1787-1788) and `check_db_subnet_group_subnets` builds the 'Current AZ coverage' list from it as well (state.rs:2359-2367), while the coverage test itself only counts DISTINCT subnet IDs (`distinct.len() < MIN_DB_SUBNET_GROUP_AVAILABILITY_ZONES`), never distinct zones. EC2 state is never consulted. In the replay, CreateDBSubnetGroup over two subnets that EC2 reports in two distinct AZs succeeded (200) and DescribeDBSubnetGroups then reported both subnets in the SAME zone: the check's positive control read `{s['SubnetAvailabilityZone']['Name'] for s in group['Subnets']}` and got a one-element set ({'us-west-2a'} in this run) where it must contain the two AZs of the supplied subnets. Two subnet ids whose hashes collide land in one zone; two subnet ids in one real AZ can be reported in two zones.

Why: AZ coverage is the fact that decides whether a DB instance or Aurora cluster can be created against the group at all and where RDS places its writer/standby. A client that creates a correct multi-AZ subnet group and then reads the group back is told it covers one zone, so Multi-AZ provisioning logic, placement assertions and CloudFormation templates that key on the group's zones all take the wrong branch; conversely a genuinely single-AZ list of two distinct subnet ids is accepted because the rule counts ids, not zones.

Fix: the subnet -> Availability Zone resolution in the rds crate's state/projection layer and the AZ-coverage precondition that shares it — the zone must be read from the account's EC2 subnet record named in SubnetIds (and the coverage rule must count DISTINCT zones), rather than being hashed from the id, in both the DBSubnetGroup/Subnet projection used by CreateDBSubnetGroup/DescribeDBSubnetGroups/ModifyDBSubnetGroup and the handler precondition. Open issue 20261104T053605Z-rds-5-net-new-causes-db-subnet-groups.md (Cause 1) and 20261104T154127Z-rds-2-net-new-causes-the-az-coverage.md (Cause 1) describe this same mis-resolution for the refusal message and for neighbouring checks, and 20261029T214632Z-rds-db-subnet-group-az-coverage-rule-is.md describes create/modify enforcement; none of them carries this check's current replay key (the message assertion this check makes now PASSES — it is the successful positive-control read-back that diverges), so this cause is filed separately and should be closed together with them.

Traces: rds.create-db-subnet-group-single-az-rejected (mismatch, assertion at check line 69 on the positive-control group's AZ set). Code: services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:1787 and :2838; services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_subnet_group.rs:177-178 and :264.

## Evidence
Failing check rds.create-db-subnet-group-single-az-rejected, verbatim replay output:
  File "check_rds_create_db_subnet_group_single_az_rejected.py", line 69, in run
    assert len(created_azs) >= 2, (
  AssertionError: positive-control subnet group 'sim2real-rds-2az-096c2c19c910' should cover >=2 AZs, got {'us-west-2a'}

Expected: after ec2.describe_subnets picks two subnets of the run's VPC in two DISTINCT AvailabilityZones and rds.create_db_subnet_group(SubnetIds=[both]) answers 200, rds.describe_db_subnet_groups(DBSubnetGroupName=<group>) reports DBSubnetGroups[0].Subnets with one entry per supplied subnet and SubnetAvailabilityZone.Name equal to that subnet's AZ as EC2 reports it — i.e. a two-element AZ set.
Actual: the group is created and described, but every Subnet carries the same SubnetAvailabilityZone.Name, so the AZ set has one element. The group name and the concrete zone letter are this run's values; the requirement is that the zone set equals the EC2 zones of the supplied subnet ids.

The negative leg of the same check passed: CreateDBSubnetGroup with a single-AZ subnet list is refused 400 DBSubnetGroupDoesNotCoverEnoughAZs with "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <AZs>. Add subnets to cover at least 2 AZs." and DescribeDBSubnetGroups on the rejected name answers DBSubnetGroupNotFoundFault "DB Subnet group '<name>' not found." — so only the AZ resolution behind the successful read-back diverges here.

Source of the divergence in the checkout: services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:2838 `pub fn subnet_availability_zone(region, subnet_id)` returns `format!("{region}{}", AVAILABILITY_ZONE_SUFFIXES[sha256(subnet_id)[0] % 3])`; state.rs:1787 uses it for Subnet$SubnetAvailabilityZone, state.rs:2359 uses it for the refusal's coverage list, and the coverage rule at state.rs:2360 compares the count of distinct subnet IDs, not of zones.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2.describe_subnets over the run's VPC and pick two subnets S1, S2 that EC2 reports in two DIFFERENT AvailabilityZones (e.g. us-west-2a and us-west-2b).
3. rds.create_db_subnet_group(DBSubnetGroupName=<new name>, DBSubnetGroupDescription='x', SubnetIds=[S1, S2]) — answers 200.
4. rds.describe_db_subnet_groups(DBSubnetGroupName=<that name>) and collect {s['SubnetAvailabilityZone']['Name'] for s in DBSubnetGroups[0]['Subnets']} — the set has ONE element instead of the two AZs EC2 reported in step 2 (the zone reported for each subnet is a hash of its id, not its EC2 zone).
5. Inverse: pick two subnets that EC2 reports in the SAME AZ and repeat step 3 — the request is accepted instead of refused with DBSubnetGroupDoesNotCoverEnoughAZs, because the coverage rule counts distinct subnet ids rather than distinct zones.
