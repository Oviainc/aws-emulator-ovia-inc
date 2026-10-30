Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-subnet-group-lifecycle
Issue-Step: setup#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-subnet-group-lifecycle rds-db-subnet-group-owned-vpc-lifecycle rds-create-db-instance-catalog-triple rds-delete-groups-in-use-by-instance
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: rds|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: rds|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: rds|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.VpcId
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.CACertificateIdentifier
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBInstanceClass
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBSubnetGroup.DBSubnetGroupDescription
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBSubnetGroup.Subnets
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.DBSubnetGroup.VpcId
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.HttpEndpointEnabled
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.StorageThroughput
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.StorageType
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.TagList
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.VPCNetworkingEnabled
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.VpcSecurityGroups
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DBSubnetGroup.DBSubnetGroupDescription
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DBSubnetGroup.Subnets
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DBSubnetGroup.VpcId
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.HttpEndpointEnabled
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.OptionGroupMemberships.OptionGroupMembership.Status
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PendingModifiedValues
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.StorageThroughput
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.StorageType
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.VPCNetworkingEnabled
Issue-Key: rds|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.VpcSecurityGroups
Issue-Related: 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md
Issue-Commit: 263537c8d0bbf6adc085b4147c3053be10a9244f

# rds DB subnet group and DB instance projections drop VpcId, subnet AZ detail and misreport storage defaults

> _Note: this issue was identified against the code base at commit `263537c8d0bbf6adc085b4147c3053be10a9244f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
Every DB-subnet-group projection, and the embedded DBSubnetGroup inside DB instance responses, is serialized from an impoverished state record: the VPC the group belongs to, each subnet's availability zone and Outpost placement, and the group's supported network types are never emitted, and subnets come back in insertion order instead of the service's order. The same instance projections also report storage and class defaults the request did not ask for. There are 5 causes to close. This is adjacent to the open issue 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md, which carries other clusters' replay keys but not these five - they are net-new here and must be closed in their own right.

Cause 1 of 5: CreateDBSubnetGroup's response omits VpcId, SupportedNetworkTypes and all per-subnet detail.
AWS: Smithy: DBSubnetGroup = {DBSubnetGroupName, DBSubnetGroupDescription, VpcId, SubnetGroupStatus, Subnets (SubnetList), DBSubnetGroupArn, SupportedNetworkTypes (values IPV4/DUAL)}; Subnet = {SubnetIdentifier, SubnetAvailabilityZone, SubnetOutpost, SubnetStatus}. The recorded create response resolves the VPC from the supplied subnets and echoes it as VpcId, reports SupportedNetworkTypes derived from the subnets' address families, and emits each Subnet with SubnetStatus, an empty SubnetOutpost element and SubnetAvailabilityZone.Name. Subnets are returned in the service's own order, not the request order.
Emulator: returns the group with VpcId and SupportedNetworkTypes absent, and each Subnet carrying only SubnetIdentifier and SubnetStatus - no SubnetAvailabilityZone, no SubnetOutpost - in request/insertion order, which also transposes the first two entries relative to the recording.
Why: a client picks a DB subnet group for a cluster/instance by checking which VPC it is in and which AZs it spans (multi-AZ placement, cross-AZ failover planning, network-type selection). With VpcId and SubnetAvailabilityZone absent that decision cannot be made at all, and a client that trusts the order sees a different AZ than AWS reports.
Fix: the DBSubnetGroup serializer and the subnet-group state record (VPC association, per-subnet AZ/Outpost, supported network types, canonical subnet ordering) used by the CreateDBSubnetGroup handler.
Traces: rds-db-subnet-group-lifecycle#setup#5 (cluster also covers rds-db-subnet-group-owned-vpc-lifecycle#task#7).

Cause 2 of 5: DescribeDBSubnetGroups reads the group back with the same members missing.
AWS: as Cause 1; the read path returns the full DBSubnetGroup including VpcId, SupportedNetworkTypes {member: IPV4} and each Subnet's SubnetAvailabilityZone.Name and SubnetOutpost.
Emulator: DescribeDBSubnetGroups answers 200 with VpcId and SupportedNetworkTypes absent and subnets reduced to {SubnetIdentifier, SubnetStatus}, in the wrong order.
Why: the read-back is what tooling actually consults (the create response is often discarded), so the gap is permanent for the life of the group rather than a one-off on create.
Fix: same serializer/state as Cause 1, on the DescribeDBSubnetGroups path.
Traces: rds-db-subnet-group-lifecycle#task#6.

Cause 3 of 5: a three-subnet group spanning two AZs is read back with the subnets re-ordered and un-zoned.
AWS: the recorded DescribeDBSubnetGroups for a group built over three subnets returns them in the service's order (subnet ...2b in us-west-2a, ...2f in us-west-2a, ...2d in us-west-2b), each with SubnetAvailabilityZone.Name and an empty SubnetOutpost.
Emulator: returns ...2b, ...2d, ...2f - insertion order - with no AZ on any entry, and again no VpcId or SupportedNetworkTypes.
Why: with three subnets over two AZs the ordering difference changes which subnet a positional client treats as primary, and the missing AZ makes the group look single-AZ, so Multi-AZ eligibility checks come out wrong.
Fix: canonical subnet ordering plus AZ/Outpost population in the subnet-group projection.
Traces: rds-db-subnet-group-owned-vpc-lifecycle#task#9.

Cause 4 of 5: DeleteDBInstance's returned DBInstance carries a stripped DBSubnetGroup, an empty VpcSecurityGroups/TagList, and storage/class values the request never set.
AWS: the delete response echoes the instance as it stood, including DBSubnetGroup with VpcId, DBSubnetGroupDescription and the full Subnets list (with AZ and Outpost), VpcSecurityGroups with one VpcSecurityGroupMembership {VpcSecurityGroupId, Status active}, the instance's TagList, VPCNetworkingEnabled=true, HttpEndpointEnabled=false, and the storage attributes actually in effect (DBInstanceClass db.t4g.micro, StorageType gp2, StorageThroughput 0, empty CACertificateIdentifier).
Emulator: omits VPCNetworkingEnabled and HttpEndpointEnabled, omits DBSubnetGroup.VpcId/.Subnets/.DBSubnetGroupDescription, serializes VpcSecurityGroups and TagList as empty strings, and reports DBInstanceClass db.t2.micro, StorageType gp3, StorageThroughput 125 and CACertificateIdentifier rds-ca-rsa2048-g1 - i.e. hard-coded defaults rather than the instance's own values.
Why: the terminal delete response is what teardown logic records; a client that asserts on the deleted instance's class/storage or that harvests its security groups or tags gets values that were never requested, and the stripped subnet group hides the instance's network placement.
Fix: the DBInstance serializer (shared subnet-group/security-group/tag projection) and the instance state record's storage/class defaulting, on the DeleteDBInstance path.
Traces: rds-create-db-instance-catalog-triple#task#15.

Cause 5 of 5: CreateDBInstance has the same defects and additionally reports the wrong option-group and pending-modification state.
AWS: the recorded create response carries DBSubnetGroup fully populated, VpcSecurityGroups with the active membership, VPCNetworkingEnabled=true, HttpEndpointEnabled=false, StorageType gp2 with StorageThroughput 0, PendingModifiedValues containing the MasterUserPassword that is still being applied, and OptionGroupMemberships[].Status='pending-apply' while the option group is still being attached.
Emulator: omits VPCNetworkingEnabled/HttpEndpointEnabled and the DBSubnetGroup detail, serializes VpcSecurityGroups and PendingModifiedValues as empty strings, reports StorageType gp3 / StorageThroughput 125, and reports OptionGroupMembership.Status='in-sync' immediately.
Why: a client that waits for PendingModifiedValues to drain or for an option group to leave pending-apply never observes the transition at all, so it cannot distinguish 'applied' from 'never pending'; and the storage attributes it reads back do not match what it asked for.
Fix: the CreateDBInstance handler's response projection and the instance state it initializes (pending-modified values, option-group membership status, storage defaults, subnet/security-group association).
Traces: rds-delete-groups-in-use-by-instance#task#8.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, rds answers).

Cause 1 - rds-db-subnet-group-lifecycle#setup#5, CreateDBSubnetGroup, 200 vs 200: DBSubnetGroup.SupportedNetworkTypes missing (expected {'member': 'IPV4'}); DBSubnetGroup.VpcId missing (expected the resolved VPC id); Subnets.Subnet[0] expected {SubnetIdentifier ...2d, SubnetStatus Active, SubnetOutpost '', SubnetAvailabilityZone {Name us-west-2b}}, got {SubnetIdentifier ...2b, SubnetStatus Active}; Subnet[1] expected ...2b/us-west-2a, got ...2d (order transposed, AZ and Outpost absent). Cluster also covers rds-db-subnet-group-owned-vpc-lifecycle#task#7.

Cause 2 - rds-db-subnet-group-lifecycle#task#6, DescribeDBSubnetGroups, 200 vs 200: identical set - SupportedNetworkTypes missing, VpcId missing, Subnet[0]/[1] reduced to {SubnetIdentifier, SubnetStatus} and transposed.

Cause 3 - rds-db-subnet-group-owned-vpc-lifecycle#task#9, DescribeDBSubnetGroups, 200 vs 200: SupportedNetworkTypes and VpcId missing; expected Subnet order ...2b(us-west-2a), ...2f(us-west-2a), ...2d(us-west-2b), got ...2b, ...2d, ...2f with no SubnetAvailabilityZone/SubnetOutpost.

Cause 4 - rds-create-db-instance-catalog-triple#task#15, DeleteDBInstance, 200 vs 200: DBInstance.VPCNetworkingEnabled missing (expected 'true'); HttpEndpointEnabled missing (expected 'false'); StorageThroughput expected '0' got '125'; CACertificateIdentifier expected '' got 'rds-ca-rsa2048-g1'; DBSubnetGroup.VpcId, .Subnets and .DBSubnetGroupDescription ('rdsenv catalog triple') missing; VpcSecurityGroups type_mismatch (expected {'VpcSecurityGroupMembership': {VpcSecurityGroupId …, Status 'active'}}, got ''); TagList type_mismatch (expected {'Tag': {'Key': 'rdsenv', 'Value': 'catalog-triple'}}, got ''); DBInstanceClass expected 'db.t4g.micro' got 'db.t2.micro'; StorageType expected 'gp2' got 'gp3'.

Cause 5 - rds-delete-groups-in-use-by-instance#task#8, CreateDBInstance, 200 vs 200: VPCNetworkingEnabled missing (expected 'true'); StorageThroughput expected '0' got '125'; HttpEndpointEnabled missing (expected 'false'); DBSubnetGroup.VpcId/.Subnets/.DBSubnetGroupDescription ('rdserr in-use probe') missing; VpcSecurityGroups type_mismatch (expected the active membership, got ''); PendingModifiedValues type_mismatch (expected {'MasterUserPassword': …}, got ''); OptionGroupMemberships.OptionGroupMembership.Status expected 'pending-apply' got 'in-sync'; StorageType expected 'gp2' got 'gp3'.

Smithy consulted: DescribeDBSubnetGroups -> DBSubnetGroupMessage {Marker, DBSubnetGroups}, errors [DBSubnetGroupNotFoundFault 404]; shape DBSubnetGroup {DBSubnetGroupName, DBSubnetGroupDescription, VpcId, SubnetGroupStatus, Subnets, DBSubnetGroupArn, SupportedNetworkTypes (IPV4|DUAL)}; shape Subnet {SubnetIdentifier, SubnetAvailabilityZone, SubnetOutpost, SubnetStatus}. All four of those Subnet members and VpcId/SupportedNetworkTypes are modeled, so their absence is a serializer/state gap, not an unmodeled extra.

## Steps to Reproduce
1. Boot the emulator and point an rds client at it.
2. Cause 1: CreateDBSubnetGroup with two subnets in different AZs; observe the response has no VpcId, no SupportedNetworkTypes, and Subnet entries without SubnetAvailabilityZone/SubnetOutpost, in request order.
3. Cause 2: DescribeDBSubnetGroups(DBSubnetGroupName=<group>) and observe the same missing members.
4. Cause 3: create a group over three subnets spanning two AZs and DescribeDBSubnetGroups; observe insertion ordering and absent AZs.
5. Cause 5: CreateDBInstance into that group with a tag and a master password; observe StorageType gp3 / StorageThroughput 125 instead of gp2/0, empty VpcSecurityGroups and PendingModifiedValues, OptionGroupMembership.Status 'in-sync' instead of 'pending-apply', and the stripped DBSubnetGroup.
6. Cause 4: DeleteDBInstance on it and observe the same stripped projection plus DBInstanceClass db.t2.micro and CACertificateIdentifier rds-ca-rsa2048-g1.
7. Re-replay the listed fixtures/steps to confirm.
