Issue-Source: trace-sweep-divergence
Issue-Trace: rds-already-exists-conflict-class
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-already-exists-conflict-class rds-create-db-instance-catalog-triple rds-db-proxy-lifecycle rds-subnet-group-az-coverage-and-naming rds-db-subnet-group-lifecycle rds-db-subnet-group-owned-vpc-lifecycle rds-db-instance-core-lifecycle rds-global-cluster-mutation-envelopes rds-identifier-validation-and-lowercasing
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBSecurityGroup|2xx->2xx|$.CreateDBSecurityGroupResponse.CreateDBSecurityGroupResult.DBSecurityGroup.VpcId
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBInstanceClass
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DBSubnetGroup.VpcId
Issue-Key: rds|DescribeDBProxies|2xx->2xx|$.DescribeDBProxiesResponse.DescribeDBProxiesResult.DBProxies.member.Auth.member.ClientPasswordAuthType
Issue-Key: rds|DescribeDBProxies|2xx->2xx|$.DescribeDBProxiesResponse.DescribeDBProxiesResult.DBProxies.member.VpcId
Issue-Key: rds|DescribeDBProxies|2xx->2xx|$.DescribeDBProxiesResponse.DescribeDBProxiesResult.DBProxies.member.VpcSecurityGroupIds
Issue-Key: rds|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*].SubnetAvailabilityZone
Issue-Key: rds|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*].SubnetOutpost
Issue-Key: rds|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: rds|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.VpcId
Issue-Key: rds|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: rds|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: rds|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.VpcId
Issue-Key: rds|DescribeValidDBInstanceModifications|2xx->2xx|$.DescribeValidDBInstanceModificationsResponse.DescribeValidDBInstanceModificationsResult.ValidDBInstanceModificationsMessage.SupportsDedicatedLogVolume
Issue-Key: rds|DescribeDBProxyTargetGroups|2xx->2xx|$.DescribeDBProxyTargetGroupsResponse.DescribeDBProxyTargetGroupsResult.TargetGroups.member.Status
Issue-Key: rds|CreateGlobalCluster|2xx->2xx|$.CreateGlobalClusterResponse.CreateGlobalClusterResult.GlobalCluster.GlobalClusterResourceId
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS omits VPC, subnet and availability-zone attributes from group, proxy and instance envelopes

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSecurityGroup

## Description
The emulator's RDS resources are not attached to a VPC: every envelope that should name a VpcId, a VPC security group or a subnet's availability zone omits it, DBSubnetGroup subnets come back unordered and stripped, and two related read-backs report a wrong value. All nine calls answer 200. 9 causes.

Cause 1 of 9: CreateDBSecurityGroup.
AWS: Smithy CreateDBSecurityGroup output is the DBSecurityGroup shape; the recording's DBSecurityGroup always carries a VpcId.
Emulator: DBSecurityGroup.VpcId is absent.
Why: a client cannot tell which VPC the security group belongs to.
Fix: the DBSecurityGroup serializer / the VPC association in RDS state.
Traces: rds-already-exists-conflict-class#task#3.

Cause 2 of 9: DescribeDBInstances after a create with an explicit subnet group.
AWS: DBInstance.DBSubnetGroup.VpcId is present, and DBInstanceClass echoes the class the create actually used.
Emulator: DBSubnetGroup.VpcId absent, and DBInstanceClass reports db.t2.micro where the recording has db.t4g.micro.
Why: the instance's network placement is unreadable and its class read-back is wrong.
Fix: the DBInstance serializer and the CreateDBInstance class handling.
Traces: rds-create-db-instance-catalog-triple#task#14.

Cause 3 of 9: DescribeDBProxies.
AWS: the DBProxy member carries VpcId, VpcSecurityGroupIds (a member list) and Auth.member.ClientPasswordAuthType.
Emulator: VpcId and ClientPasswordAuthType absent, VpcSecurityGroupIds serialized as an empty string instead of the list.
Why: a client cannot discover the proxy's VPC, its security groups or its client auth type.
Fix: the DBProxy serializer and the proxy's VPC/auth state.
Traces: rds-db-proxy-lifecycle#task#7.

Cause 4 of 9: CreateDBSubnetGroup.
AWS: DBSubnetGroup carries VpcId, SupportedNetworkTypes (member 'IPV4'), and each Subnet carries SubnetOutpost (empty) and SubnetAvailabilityZone.Name.
Emulator: SupportedNetworkTypes, VpcId, SubnetOutpost and SubnetAvailabilityZone are all absent.
Why: a client cannot verify AZ coverage or network type from the create response.
Fix: the DBSubnetGroup serializer and the subnet metadata in RDS state.
Traces: rds-subnet-group-az-coverage-and-naming#task#9; also #task#16, #task#17.

Cause 5 of 9: ModifyDBSubnetGroup.
AWS: Smithy ModifyDBSubnetGroup output DBSubnetGroup; the recording carries VpcId, SupportedNetworkTypes and per-Subnet SubnetOutpost/SubnetAvailabilityZone, with the subnets in the recorded order.
Emulator: those members are absent and the Subnet list order differs from the recording.
Why: same, plus an order-sensitive client reads a different subnet at a given index.
Fix: the ModifyDBSubnetGroup response serializer and its subnet ordering.
Traces: rds-db-subnet-group-lifecycle#task#8.

Cause 6 of 9: ModifyDBSubnetGroup on a customer-owned VPC with three subnets.
AWS: same members present, subnets in the recorded order.
Emulator: same members absent and the three subnets permuted.
Why: same.
Fix: same handler.
Traces: rds-db-subnet-group-owned-vpc-lifecycle#task#8.

Cause 7 of 9: DescribeValidDBInstanceModifications.
AWS: ValidDBInstanceModificationsMessage.SupportsDedicatedLogVolume is 'false' for the recorded instance.
Emulator: reports 'true'.
Why: a client reads a modification capability the instance does not have.
Fix: the DescribeValidDBInstanceModifications handler's capability derivation.
Traces: rds-db-instance-core-lifecycle#task#7.

Cause 8 of 9: DescribeDBProxyTargetGroups after the target group settles.
AWS: TargetGroups.member.Status is 'available'.
Emulator: 'creating' — the target group never leaves its transient state.
Why: a client polling for an available target group loops to its timeout.
Fix: the DB proxy target-group state transition.
Traces: rds-db-proxy-lifecycle#task#10.

Cause 9 of 9: CreateGlobalCluster's GlobalClusterResourceId.
AWS: GlobalCluster.GlobalClusterResourceId matches the recorded global-cluster resource-id shape.
Emulator: emits a DB-cluster-style id ('cluster-' + digits), which fails that shape.
Why: a client that uses GlobalClusterResourceId as a global-cluster handle (for example to look up automated backups) passes a value RDS would not recognise. The divergence is on the value's shape, not on a particular generated id.
Fix: the CreateGlobalCluster handler's resource-id minting.
Traces: rds-already-exists-conflict-class#task#5; also rds-global-cluster-mutation-envelopes#task#2, rds-identifier-validation-and-lowercasing#task#9.

## Evidence
Replay cluster details (status 200 matched in all nine):
1. rds-already-exists-conflict-class#task#3 CreateDBSecurityGroup: DBSecurityGroup.VpcId missing.
2. rds-create-db-instance-catalog-triple#task#14 DescribeDBInstances: DBInstance.DBInstanceClass expected 'db.t4g.micro' got 'db.t2.micro'; DBInstance.DBSubnetGroup.VpcId missing (expected 'vpc-00000000000000022').
3. rds-db-proxy-lifecycle#task#7 DescribeDBProxies: member.VpcSecurityGroupIds type_mismatch (expected {'member': 'sg-...'}, got ''); member.Auth.member.ClientPasswordAuthType missing (expected 'MYSQL_CACHING_SHA2_PASSWORD'); member.VpcId missing.
4. rds-subnet-group-az-coverage-and-naming#task#9 CreateDBSubnetGroup: DBSubnetGroup.SupportedNetworkTypes missing (expected {'member': 'IPV4'}); .VpcId missing; Subnets.Subnet[0]/[1].SubnetOutpost missing; Subnets.Subnet[0]/[1].SubnetAvailabilityZone missing (expected {'Name': 'us-west-2a'} / {'Name': 'us-west-2b'}).
5. rds-db-subnet-group-lifecycle#task#8 ModifyDBSubnetGroup: SupportedNetworkTypes and VpcId missing; Subnet[0]/[1] reported both missing and extra — the emulator returns {SubnetIdentifier, SubnetStatus} only, in the opposite order to the recording.
6. rds-db-subnet-group-owned-vpc-lifecycle#task#8 ModifyDBSubnetGroup: same, over three subnets.
7. rds-db-instance-core-lifecycle#task#7 DescribeValidDBInstanceModifications: SupportsDedicatedLogVolume expected 'false' got 'true'.
8. rds-db-proxy-lifecycle#task#10 DescribeDBProxyTargetGroups: TargetGroups.member.Status expected 'available' got 'creating'.
9. rds-already-exists-conflict-class#task#5 CreateGlobalCluster: GlobalCluster.GlobalClusterResourceId value_mismatch — expected the recorded shape, got 'cluster-00000000000000000000000001'.
(vpc-/sg-/subnet- ids and the resource id above are this run's generated values; the requirement is that the members be present and shaped as the recording asserts.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-already-exists-conflict-class#task#3 and every step in trace_ids; each creates or reads a security group, subnet group, proxy, instance or global cluster and the replay diffs the envelope's VPC/subnet/AZ members.
