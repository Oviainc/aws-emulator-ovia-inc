Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.broker-count-decrease-rejected kafka.cluster-create-active-transition kafka.cluster-create-invalid-broker-config-rejected kafka.cluster-create-invalid-instance-type-rejected kafka.cluster-create-subnets-same-az-rejected kafka.cluster-create-unsupported-kafka-version-rejected kafka.cluster-create-v2-both-cluster-types-rejected kafka.cluster-delete-stale-version-rejected kafka.cluster-duplicate-name-conflict-across-api-versions kafka.cluster-mutating-updates-rejected-while-creating kafka.cluster-operations-and-policy-views-on-new-cluster kafka.cluster-policy-concurrent-optimistic-lock kafka.cluster-policy-put-get-delete-lifecycle kafka.cluster-policy-version-mismatch-rejected kafka.cluster-version-config-updates-rejected-while-creating kafka.clusterv2-provisioned-create-active kafka.clusterv2-serverless-create-active kafka.e2e-replicator-cross-cluster-topic-sync-teardown kafka.list-nodes-reflects-cluster-brokers kafka.reboot-broker-state-transition kafka.replicator-create-invalid-role-rejected kafka.replicator-create-running-transition kafka.replicator-delete-transition kafka.replicator-nonexistent-cluster-not-found kafka.scram-secret-associate-list-disassociate kafka.serverless-bootstrap-broker-endpoint-set kafka.tag-resource-round-trip kafka.topic-create-describe-delete-lifecycle kafka.topic-create-duplicate-exists kafka.topic-delete-unknown-topic kafka.topic-describe-partitions-matches-count kafka.topic-partition-decrease-rejected kafka.topic-replication-factor-exceeds-brokers-rejected kafka.update-broker-count-concurrent-version-conflict kafka.update-broker-count-operation-lifecycle kafka.update-monitoring-server-enforced kafka.update-rebalancing-server-enforced kafka.update-replication-info-invalid-target-topics-rejected kafka.vpc-connection-create-available-lifecycle kafka.vpcconnection-requires-multi-vpc-enabled-cluster
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.broker-count-decrease-rejected|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-create-active-transition|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-create-invalid-broker-config-rejected|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-create-invalid-instance-type-rejected|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-create-subnets-same-az-rejected|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-create-unsupported-kafka-version-rejected|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-create-v2-both-cluster-types-rejected|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-delete-stale-version-rejected|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-duplicate-name-conflict-across-api-versions|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-mutating-updates-rejected-while-creating|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-operations-and-policy-views-on-new-cluster|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-policy-concurrent-optimistic-lock|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-policy-put-get-delete-lifecycle|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-policy-version-mismatch-rejected|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.cluster-version-config-updates-rejected-while-creating|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.clusterv2-provisioned-create-active|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.clusterv2-serverless-create-active|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.e2e-replicator-cross-cluster-topic-sync-teardown|_discover_network#5|AssertionError|raise AssertionError(f"no VPC in this account has >= {num_subnets} subnets in distinct AZs")
Issue-Key: kafka.list-nodes-reflects-cluster-brokers|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.reboot-broker-state-transition|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.replicator-create-invalid-role-rejected|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.replicator-create-running-transition|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.replicator-delete-transition|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.replicator-nonexistent-cluster-not-found|_discover_network#3|AssertionError|raise AssertionError(
Issue-Key: kafka.scram-secret-associate-list-disassociate|_discover_network#4|AssertionError|raise AssertionError(
Issue-Key: kafka.serverless-bootstrap-broker-endpoint-set|_discover_network#2|AssertionError|raise AssertionError("no VPC with >=2 subnets in distinct standard AZs and a security group")
Issue-Key: kafka.tag-resource-round-trip|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.topic-create-describe-delete-lifecycle|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.topic-create-duplicate-exists|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.topic-delete-unknown-topic|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.topic-describe-partitions-matches-count|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.topic-partition-decrease-rejected|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.topic-replication-factor-exceeds-brokers-rejected|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.update-broker-count-concurrent-version-conflict|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.update-broker-count-operation-lifecycle|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.update-monitoring-server-enforced|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.update-rebalancing-server-enforced|_discover_subnets#7|AssertionError|raise AssertionError(
Issue-Key: kafka.update-replication-info-invalid-target-topics-rejected|_default_vpc_subnets_sg#2|AssertionError|raise AssertionError(
Issue-Key: kafka.vpc-connection-create-available-lifecycle|_discover_network#2|AssertionError|raise AssertionError(
Issue-Key: kafka.vpcconnection-requires-multi-vpc-enabled-cluster|_discover_network#2|AssertionError|raise AssertionError(
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# MSK cluster provisioning is unreachable: the emulator's EC2 account state holds no VPCs or subnets, so every MSK check that needs a network aborts in discovery

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
ONE root cause, affecting 40 kafka checks.

Cause 1 of 1 — an unfiltered EC2 DescribeSubnets answers with an empty inventory, so there is no VPC in the account that has two subnets in two distinct standard Availability Zones, and MSK cluster creation (which REQUIRES ClientSubnets in >= 2 AZs) can never be expressed.

AWS: recorded behavior only. In a normal account/region, ec2.DescribeSubnets() with no Filters returns the default VPC's subnets — one default subnet per standard Availability Zone of the region, State 'available', OwnerId equal to the caller's account. That is the ambient prerequisite every MSK provisioning path uses: BrokerNodeGroupInfo.ClientSubnets must name subnets in at least two distinct AZs (Smithy: kafka#CreateCluster input BrokerNodeGroupInfo.ClientSubnets, required). Nothing in the kafka model describes the network inventory itself, so the requirement here is EC2's: a DescribeSubnets for a fresh account/region is non-empty.

Emulator: ec2.DescribeSubnets() returns HTTP 200 with an empty Subnets list — the operation IS implemented and reads crate state, but the EC2 store has no subnets (and no VPCs) seeded, and there is no way to create one either (EC2 CreateVpc answers 501 NotImplemented; that stub is already filed and the suite reports it blocking 76 of these checks on a different path). The checks therefore abort in their network-discovery helper with AssertionError 'no VPC with >=2 subnets in distinct standard AZs owned by this account' (variants: 'no VPC in this account has >= 3 subnets in distinct AZs', 'no VPC with >=2 subnets in distinct standard AZs and a security group', '... to provision test clusters') before a single kafka call is made.

Why: a client relying on AWS behavior bootstraps MSK from the ambient default VPC — this is the documented quick-start path. Against the emulator it gets an empty subnet list, so it cannot build a CreateCluster/CreateClusterV2 request at all. Every provisioned-cluster behavior (state transitions, broker count/storage/type updates, cluster policy, SCRAM secrets, topics, nodes, replicators, VPC connections, bootstrap brokers) is unreachable, and the kafka suite's pass rate is dominated by this one gap rather than by MSK fidelity.

Fix: EC2 account-state seeding in services/service-com-amazonaws-ec2-amazonec2 (src/state.rs) must present a default VPC with one default subnet per standard AZ of the region, owned by the caller's account, State 'available', so the implemented reader services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_subnets.rs has rows to report; a security group for that VPC must be reachable the same way. A related open issue, 20261022T154613Z-eks-clusters-cannot-be-provisioned-at-all-no.md, names the SAME default-VPC gap as its Cause 1, but it is keyed to eks checks and to the 'default-for-az'-filtered read only; it does not carry any of the kafka replay keys below, and the kafka checks use an UNFILTERED DescribeSubnets, so this one must be closed and re-replayed separately.

Traces: the 40 kafka check ids listed in trace_ids; representative ones are kafka.clusterv2-provisioned-create-active, kafka.cluster-create-active-transition, kafka.topic-create-describe-delete-lifecycle, kafka.serverless-bootstrap-broker-endpoint-set, kafka.e2e-replicator-cross-cluster-topic-sync-teardown.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/192 passed; kafka 0/191.

Expected (real AWS): ec2.describe_subnets() -> Subnets non-empty, with >= 2 entries whose AvailabilityZone values are distinct standard AZs of the region (e.g. us-west-2a/us-west-2b), State 'available'. The MSK check then calls kafka.create_cluster(BrokerNodeGroupInfo={'InstanceType':'kafka.m5.large','ClientSubnets':[<2 subnets>],'StorageInfo':{'EbsStorageInfo':{'VolumeSize':10}}}, KafkaVersion='3.9.x', NumberOfBrokerNodes=2).

Actual (emulator), 40 checks, identical shape — e.g.
kafka.broker-count-decrease-rejected:
  File "check_kafka_broker_count_decrease_rejected.py", line 76, in _discover_network
    raise AssertionError("no VPC with >=2 subnets in distinct standard AZs owned by this account")
  AssertionError: no VPC with >=2 subnets in distinct standard AZs owned by this account
kafka.e2e-replicator-cross-cluster-topic-sync-teardown:
  AssertionError: no VPC in this account has >= 3 subnets in distinct AZs
kafka.serverless-bootstrap-broker-endpoint-set:
  AssertionError: no VPC with >=2 subnets in distinct standard AZs and a security group
kafka.update-replication-info-invalid-target-topics-rejected:
  AssertionError: no VPC with >=2 subnets in distinct standard AZs owned by this account to provision test clusters

The helper wraps describe_subnets in try/except and would have reported 'cannot discover EC2 networking for MSK: <exc>' had the call failed, so DescribeSubnets returned 200 with zero qualifying rows rather than erroring. services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_subnets.rs:208 iterates store.subnets, i.e. the store itself is empty. The fallback of minting a network is closed too: ec2 CreateVpc answers 501 'operation CreateVpc is scaffolded but not implemented in this emulator'.

## Steps to Reproduce
1. Boot the emulator.
2. ec2.describe_subnets() with no arguments -> expect a non-empty Subnets list containing one default subnet per standard AZ of the region, each State 'available' and OwnerId == sts.get_caller_identity()['Account']; observe {'Subnets': []}.
3. ec2.describe_vpcs() -> expect the account's default VPC; observe no VPC.
4. Consequently kafka.create_cluster(ClusterName='x', KafkaVersion='3.9.x', NumberOfBrokerNodes=2, BrokerNodeGroupInfo={'InstanceType':'kafka.m5.large','ClientSubnets':[<two subnets in distinct AZs>],'StorageInfo':{'EbsStorageInfo':{'VolumeSize':10}}}) cannot be formed — there are no subnet ids to pass.
5. Re-replay the 40 check ids in trace_ids after seeding a default VPC + per-AZ default subnets.
