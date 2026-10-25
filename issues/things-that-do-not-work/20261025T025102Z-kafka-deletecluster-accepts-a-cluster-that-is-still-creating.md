Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-delete-and-broker-ops-rejected-while-creating
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-delete-and-broker-ops-rejected-while-creating|_refusal#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 19ca13517733601aaaac557341193877c880d409

# DeleteCluster accepts a cluster that is still CREATING instead of refusing it with BadRequestException

> _Note: this issue was identified against the code base at commit `19ca13517733601aaaac557341193877c880d409`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCluster

## Description
Single cause (Cause 1 of 1): the MSK cluster-state guard is missing from DeleteCluster, so a destructive call that real MSK refuses while the cluster is provisioning is accepted and the cluster is removed from the registry.

AWS: Smithy (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json) models DeleteCluster with input {ClusterArn (httpLabel, required), CurrentVersion (httpQuery)} and errors BadRequestException (HTTP 400), ForbiddenException (403), InternalServerErrorException (500), NotFoundException (404). The lifecycle rule itself is recorded behavior only: a cluster whose State is CREATING refuses DeleteCluster with HTTP 400 / error code `BadRequestException`; the recorded message, which is an example and not a requirement, is "You can't delete cluster in CREATING state." The refusal must have no effect — a later DescribeClusterV2 still reports State CREATING, and the cluster goes on to settle in ACTIVE, never DELETING.

Emulator: DeleteCluster answers HTTP 200 with {"ClusterArn": "arn:aws:kafka:<region>:<account>:cluster/<name>/<uuid>-1", "State": "DELETING"} for a cluster the emulator itself had just reported in State CREATING, and the cluster is unregistered, so the delete takes effect. The handler resolves the cluster through the settling lookup and checks only the ARN shape and the optional CurrentVersion; it never inspects the cluster's State before calling state.unregister_cluster.

Why: a client that relies on MSK's provisioning-window guard loses its safety net. Code that probes an in-flight cluster — a retry after a timed-out create, a teardown racing a create, an idempotent "delete if wrong shape" path — gets a success and a vanished cluster where real MSK returns 400 and leaves the cluster to finish provisioning. Tests written to assert the refusal pass against AWS and fail here, and the opposite direction (cleanup logic that waits for the cluster to leave CREATING before deleting, because AWS forces it to) is never exercised.

Fix: belongs in the DeleteCluster handler's pre-conditions — add the cluster-state guard after the settled_cluster_in lookup and before unregister_cluster, returning BadRequestException (400) with invalid_parameter ClusterArn when the settled cluster's state is not a state MSK allows a delete from (CREATING in particular), leaving the registry untouched. The same guard family is what the check's other probes (ListNodes, GetBootstrapBrokers, RebootBroker) need; those operations are unimplemented stubs and are tracked separately (20261024T221708Z-kafka-listnodes-stub.md, 20261022T171242Z-kafka-rebootbroker-stub.md), so this issue is only about DeleteCluster's missing guard.

Traces: services/service-com-amazonaws-kafka-kafka/src/ops/delete_cluster.rs lines 172-213 (lookup, version check, unregister — no state check); state settling at services/service-com-amazonaws-kafka-kafka/src/state.rs (CLUSTER_ACTIVATION_DELAY, Cluster::settle). Failing check: kafka.cluster-delete-and-broker-ops-rejected-while-creating.

## Evidence
Check kafka.cluster-delete-and-broker-ops-rejected-while-creating [mismatch]:

The check creates its own VPC/subnets/SG and a 2-broker PROVISIONED cluster via CreateClusterV2, asserts CreateClusterV2 answered State=CREATING and that DescribeClusterV2 also reports CREATING, then probes DeleteCluster expecting a refusal.

Expected (AWS): ClientError with Error.Code == "BadRequestException" (HTTP 400); example message "You can't delete cluster in CREATING state."; cluster still CREATING afterwards and settling in ACTIVE.

Actual (emulator):
  AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ClusterArn': 'arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-behav-guard-<id>/<uuid>-1', 'State': 'DELETING'}

Source confirmation — services/service-com-amazonaws-kafka-kafka/src/ops/delete_cluster.rs:
  - line 160: ARN-shape check only
  - lines 177-189: settled_cluster_in lookup -> NotFoundException when absent
  - lines 194-207: optional CurrentVersion match
  - line 209: state.unregister_cluster(&input.cluster_arn) — reached unconditionally for a CREATING cluster
  - lines 210-213: Ok(DeleteClusterOutput { state: Some("DELETING") })
No branch anywhere inspects cluster.state. The file's own header documents the shortcut ("describe after delete answers 404 ... rather than the real service's longer-lived DELETING window") but no state guard.

## Steps to Reproduce
1. Build and boot the emulator, point an AWS SDK/botocore client at it (service `kafka`).
2. Create a VPC with two subnets in distinct AZs and a security group.
3. cluster = kafka.create_cluster_v2(ClusterName=<unique>, Provisioned={BrokerNodeGroupInfo: {InstanceType: "kafka.m5.large", ClientSubnets: <the two subnets>, SecurityGroups: [<sg>], StorageInfo: {EbsStorageInfo: {VolumeSize: 10}}}, KafkaVersion: "3.9.x", NumberOfBrokerNodes: 2})
4. Immediately (within the provisioning window) assert kafka.describe_cluster_v2(ClusterArn=cluster["ClusterArn"])["ClusterInfo"]["State"] == "CREATING".
5. Call kafka.delete_cluster(ClusterArn=cluster["ClusterArn"]).
   Observed: HTTP 200, {"State": "DELETING"}, and a subsequent DescribeClusterV2 answers NotFoundException.
   Expected: HTTP 400 BadRequestException, cluster unchanged and still CREATING, later settling in ACTIVE.
