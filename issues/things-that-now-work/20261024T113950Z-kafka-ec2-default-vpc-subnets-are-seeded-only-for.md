Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-duplicate-name-conflict-across-api-versions kafka.cluster-mutating-updates-rejected-while-creating kafka.cluster-operations-and-policy-views-on-new-cluster kafka.cluster-version-config-updates-rejected-while-creating kafka.clusterv2-provisioned-create-active kafka.clusterv2-serverless-create-active kafka.e2e-replicator-cross-cluster-topic-sync-teardown kafka.replicator-create-running-transition kafka.replicator-delete-transition kafka.replicator-nonexistent-cluster-not-found kafka.tag-resource-round-trip kafka.tag-unicode-and-empty-value-preserved kafka.update-broker-count-operation-lifecycle kafka.update-replication-info-invalid-target-topics-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-duplicate-name-conflict-across-api-versions|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-mutating-updates-rejected-while-creating|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-operations-and-policy-views-on-new-cluster|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-version-config-updates-rejected-while-creating|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.clusterv2-provisioned-create-active|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.clusterv2-serverless-create-active|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.e2e-replicator-cross-cluster-topic-sync-teardown|_discover_network#5|AssertionError|raise AssertionError(f"no VPC in this account has >= {num_subnets} subnets in distinct AZs")
Issue-Key: kafka.replicator-create-running-transition|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.replicator-delete-transition|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.replicator-nonexistent-cluster-not-found|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.tag-resource-round-trip|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.tag-unicode-and-empty-value-preserved|_default_client_subnets#2|AssertionError|raise AssertionError(
Issue-Key: kafka.update-broker-count-operation-lifecycle|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.update-replication-info-invalid-target-topics-rejected|_default_vpc_subnets_sg#2|AssertionError|raise AssertionError(
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# EC2 default-VPC subnets are seeded only for us-east-1, so MSK network discovery finds no subnet in the request's region

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
ONE root cause, blocking 14 kafka checks before a single MSK call is made.

Cause 1 of 1 — the ambient default VPC's default subnets are seeded in a FIXED region's Availability Zones (us-east-1) rather than in the zones of the region the request is signed for, so an unfiltered DescribeSubnets made in any other region reports only subnets whose AvailabilityZone belongs to a different region.

AWS: recorded behavior only (nothing in the kafka or ec2 Smithy model describes the account's ambient network inventory). In a normal account, every region in which the account has a default VPC answers ec2.DescribeSubnets() with that region's default subnets — one default subnet per standard Availability Zone OF THE REGION THE REQUEST WAS MADE IN, State 'available', OwnerId equal to the caller's account (Amazon VPC User Guide, "Default VPCs": "When we create a default VPC, we create a default subnet in each Availability Zone in the Region"). This is the ambient prerequisite of every MSK provisioning path: Smithy kafka#CreateCluster requires BrokerNodeGroupInfo.ClientSubnets, and the cluster's brokers must sit in at least two distinct AZs of the region the cluster is created in.

Emulator: ec2.DescribeSubnets() answers HTTP 200 with subnets that all live in us-east-1 zones, even when the request is signed for us-west-2 (the region every check in this run used — see the us-west-2 ARNs the kafka responses carry). The store is seeded once, per ACCOUNT rather than per region: services/service-com-amazonaws-ec2-amazonec2/src/state.rs `impl Default for ComAmazonawsEc2Amazonec2State` calls `insert_default_subnets(&default_vpc, DEFAULT_VPC_CIDR_BLOCK, DEFAULT_REGION)`, and `DEFAULT_REGION` is `EC2_INTERNAL_REGION` = "us-east-1" (same file). The module docs state the consequence outright: "This store is per ACCOUNT, not per Region … so the seeded subnets are the ones of DEFAULT_REGION." DescribeSubnets (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_subnets.rs) reports exactly that store, so a caller in us-west-2 sees zero subnets in any us-west-2 zone and the checks abort in their network-discovery helper with AssertionError 'no VPC with >=2 subnets in distinct standard AZs owned by this account' (variants: '… >= 3 subnets in distinct AZs', '… to provision test clusters', 'no VPC owned by this account has >= 2 subnets in distinct standard AZs').

NOTE for the fixer: the fixed issue 20261022T171242Z-kafka-msk-cluster-provisioning-is-unreachable-the-emulator-s.md asked for exactly "a default VPC with one default subnet per standard AZ of the region". The seeding it added exists but is region-fixed, so the requirement is still unmet for every region except us-east-1 and none of the replay keys below pass. This is not a refusal — DescribeSubnets answers 200 — so it is filed as a fresh cause rather than as a reopen of a refused call.

Why: a client relying on AWS behavior bootstraps MSK (and anything else VPC-bound) from the ambient default VPC of the region it is working in. Against the emulator it can discover no subnet of its own region, so it cannot even FORM a CreateCluster/CreateClusterV2 request, and every provisioned-cluster behavior behind it (create transitions, broker-count/storage updates, cluster operations and policy views, tagging, replicators) is unreachable. The same gap hits any other service whose create takes subnet ids.

Fix: EC2 account-state seeding in services/service-com-amazonaws-ec2-amazonec2/src/state.rs — the default VPC's default subnets (and the VPC's default security group) must be present for the region the request is signed for, not for one compile-time region: either seed per region on first use of that region's credential scope, or derive the seeded network's zones from the request's region through the existing `availability_zones_in_region` fact rather than from `DEFAULT_REGION`. The reader (src/ops/describe_subnets.rs) needs no change.

Traces: the 14 check ids in trace_ids; representative ones are kafka.clusterv2-provisioned-create-active, kafka.tag-resource-round-trip, kafka.e2e-replicator-cross-cluster-topic-sync-teardown.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, 11/85 passed; kafka 11/84.

Expected (real AWS, request signed for us-west-2): ec2.describe_subnets() -> Subnets containing >= 2 entries whose AvailabilityZone values are distinct standard zones of us-west-2 (us-west-2a/us-west-2b/…), State 'available', OwnerId == sts.get_caller_identity()['Account'].

Actual (emulator), 14 checks, identical shape:
kafka.clusterv2-provisioned-create-active:
  File "check_kafka_clusterv2_provisioned_create_active.py", line 88, in _create_cluster_network
    ctx._kafka_net = _discover_network(ctx, minimum_subnets=2)
  AssertionError: no VPC with >=2 subnets in distinct standard AZs owned by this account
kafka.e2e-replicator-cross-cluster-topic-sync-teardown:
  AssertionError: no VPC in this account has >= 3 subnets in distinct AZs
kafka.tag-unicode-and-empty-value-preserved:
  AssertionError: no VPC owned by this account has >= 2 subnets in distinct standard AZs
kafka.update-replication-info-invalid-target-topics-rejected:
  AssertionError: no VPC with >=2 subnets in distinct standard AZs owned by this account to provision test clusters

The discovery helper wraps describe_subnets in try/except and would have reported 'cannot discover EC2 networking for MSK: <exc>' had the call errored, so the call returned 200; its filter is `State == 'available'` AND `AvailabilityZone.startswith(ctx.region) and len(zone) == len(ctx.region) + 1` AND `OwnerId` defaulting to the caller's account. The region in play is us-west-2 (the same run's kafka responses carry arn:aws:kafka:us-west-2:000000000000:… and a replicator refusal reads 'No replicator exists in us-west-2 …'), while state.rs seeds the default subnets with DEFAULT_REGION = "us-east-1", so the zone predicate is what excludes every row.

## Steps to Reproduce
1. Boot the emulator.
2. With credentials signed for us-west-2, call ec2.describe_subnets() with no arguments -> expect >= 2 subnets whose AvailabilityZone values are distinct us-west-2 zones, State 'available'; observe only subnets in us-east-1 zones (none usable for a us-west-2 request).
3. ec2.describe_vpcs() + ec2.describe_security_groups(Filters=[{'Name':'group-name','Values':['default']}]) in us-west-2 -> expect the region's default VPC and its default group.
4. Consequently kafka.create_cluster(ClusterName='x', KafkaVersion='3.9.x', NumberOfBrokerNodes=2, BrokerNodeGroupInfo={'InstanceType':'kafka.m5.large','ClientSubnets':[<two us-west-2 subnets>],'StorageInfo':{'EbsStorageInfo':{'VolumeSize':10}}}) cannot be formed — there are no subnet ids of the request's region to pass.
5. Re-replay the 14 check ids in trace_ids.
