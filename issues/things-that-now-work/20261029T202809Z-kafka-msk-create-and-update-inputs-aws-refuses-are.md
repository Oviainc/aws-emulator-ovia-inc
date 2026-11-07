Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-create-unsupported-kafka-version-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-create-unsupported-kafka-version-rejected kafka-rejected-provisioning-requests kafka-replicator-notfound-errors kafka-replicator-validation-errors kafka-replicator-create-describe-delete-lifecycle kafka-create-cluster-v2-validation-errors kafka-operate-on-nonexistent-replicator kafka-replicator-crud-notfound-error-paths
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateClusterV2|4xx->2xx|$.invalidParameter
Issue-Key: kafka|CreateClusterV2|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateClusterV2|4xx->2xx|status
Issue-Key: kafka|CreateCluster|4xx->2xx|$.invalidParameter
Issue-Key: kafka|CreateCluster|4xx->2xx|$.message
Issue-Key: kafka|CreateCluster|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateCluster|4xx->2xx|status
Issue-Key: kafka|CreateReplicator|4xx->2xx|$.invalidParameter
Issue-Key: kafka|UpdateReplicationInfo|4xx->2xx|$.message
Issue-Key: kafka|UpdateReplicationInfo|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kafka|UpdateReplicationInfo|4xx->2xx|status
Issue-Key: kafka|CreateClusterV2|4xx->4xx|$.invalidParameter
Issue-Key: kafka|CreateClusterV2|4xx->4xx|$.message
Issue-Key: kafka|UpdateReplicationInfo|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateReplicationInfo|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|UpdateReplicationInfo|4xx->4xx|status
Issue-Related: 20261029T202809Z-kafka-msk-invalid-value-refusals-on-operation-secret-vpc.md
Issue-Related: 20261029T202809Z-kafka-msk-resource-authorization-is-never-evaluated-so-denied.md
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK create and update inputs AWS refuses are accepted, and refusal order is wrong (8 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterV2

## Description
EIGHT distinct root causes on MSK's create/update input gates. Four of them answer HTTP 200 and MINT STATE for a request AWS refuses with 400 - the fail-open direction, where a client's negative test passes and the emulator's state is left holding a resource AWS would never have created. Two answer the right 400 with the wrong parameter and sentence, and two evaluate the gates in the wrong ORDER, so a 404 hides the 400 AWS reports. All eight must be closed.

Cause 1 of 8 - CreateClusterV2 accepts an unsupported Apache Kafka version.
AWS: Smithy kafka#CreateClusterV2 (POST /api/v2/clusters) models BadRequestException (400). A `provisioned.kafkaVersion` MSK does not offer is refused 400 BadRequestException with `invalidParameter` 'kafkaVersion'.
Emulator: HTTP 200 - the cluster is created, with no error body and no invalidParameter at all.
Why: fail-open. A client that probes supported versions by attempting a create gets a cluster on an impossible version, and the emulator's catalog and its create path disagree.
Fix: validate provisioned.kafkaVersion against the version list ListKafkaVersions serves, in services/service-com-amazonaws-kafka-kafka/src/ops/create_cluster_v2.rs, before any state is written.
Traces: kafka-cluster-create-unsupported-kafka-version-rejected#task#1.

Cause 2 of 8 - CreateCluster accepts client subnets that do not exist.
AWS: Smithy kafka#CreateCluster (POST /v1/clusters) requires BrokerNodeGroupInfo and models BadRequestException (400). ClientSubnets naming subnets the account does not hold is refused 400 BadRequestException with `invalidParameter` 'brokerNodeGroupInfo' and `message` exactly "One or more of the specified client subnets don't exist." (apostrophe U+2019) - all fixed text; the subnet ids are NOT echoed.
Emulator: HTTP 200, cluster minted, no error body.
Why: fail-open, and it means provisioned clusters exist in the emulator with no backing network, so every later network-dependent read is fiction.
Fix: resolve ClientSubnets against the account's EC2 subnets in ops/create_cluster.rs before minting. (The open behavioral issue 20261029T175822Z-kafka-createcluster-accepts-brokernodegroupinfo-clientsubnets-that-do-not-exist.md describes this same fault from the behavioral suite; it carries no wire fixture#step key, so this replay binding is not covered by it.)
Traces: kafka-rejected-provisioning-requests#task#1.

Cause 3 of 8 - CreateReplicator accepts a subnet list outside MSK's allowed count.
AWS: Smithy kafka#CreateReplicator models BadRequestException (400). A kafkaClusters[].vpcConfig.subnetIds list with too few or too many entries is refused 400 BadRequestException with `invalidParameter` 'subnetIds' and `message` exactly "Too many or too few subnet ids specified." - all fixed text; neither the count nor the limits are rendered.
Emulator: HTTP 200, replicator minted, no error body.
Why: fail-open; the replicator is then described as RUNNING over a VPC configuration MSK would have rejected.
Fix: validate subnetIds cardinality per Kafka cluster entry in ops/create_replicator.rs.
Traces: kafka-replicator-notfound-errors#task#4.

Cause 4 of 8 - UpdateReplicationInfo has no replicator-state gate.
AWS: Smithy kafka#UpdateReplicationInfo (PUT /replication/v1/replicators/{ReplicatorArn}/replication-info) models BadRequestException (400). Called on a replicator that is not in a state that accepts updates, it is refused 400 with `message` of the form "Invalid parameter replicatorArn: This operation is only valid for resources that are in one of the following states: [<the states that do accept it>]". Fixed text: "Invalid parameter replicatorArn: This operation is only valid for resources that are in one of the following states: [" and "]"; the bracketed state list is service-defined (the recording shows [RUNNING]).
Emulator: HTTP 200 - the update is applied regardless of the replicator's state.
Why: fail-open on a state machine. A client that serialises updates behind this refusal issues them concurrently here, and the replicator's state transitions diverge permanently.
Fix: gate on the stored replicator's state in ops/update_replication_info.rs before applying any change.
Traces: kafka-replicator-create-describe-delete-lifecycle#task#5.

Cause 5 of 8 - CreateClusterV2 with NEITHER cluster type blames the wrong parameter and rewords the rule.
AWS: CreateClusterV2 input has optional `provisioned` and `serverless`; exactly one is required. With neither present the refusal is 400 BadRequestException with `invalidParameter` 'clusterType' and `message` exactly "You must specify a cluster type, either provisioned or serverless." - all fixed text.
Emulator: `invalidParameter` 'Provisioned' and "A create names one cluster type: either Provisioned or Serverless must be given." - it blames one of the two members rather than the abstract 'clusterType' MSK names, and uses its own sentence with the Smithy member casing.
Why: the client is pointed at `provisioned` when `serverless` is an equally valid fix, so automated remediation picks the wrong member.
Fix: the mutual-requirement check in ops/create_cluster_v2.rs.
Traces: kafka-create-cluster-v2-validation-errors#task#1.

Cause 6 of 8 - CreateClusterV2 with BOTH cluster types uses the wrong refusal class of message.
AWS: with both `provisioned` and `serverless` present the refusal is 400 with `message` exactly "Invalid request body" - MSK treats this as a body-level rejection, NOT a per-parameter one, and says nothing about which member to drop.
Emulator: "A create names one cluster type: Provisioned and Serverless cannot both be given."
Why: Causes 5 and 6 are two DIFFERENT messages in AWS (a named-parameter refusal and an opaque body refusal); the emulator collapses them into two variants of its own sentence, so a client cannot tell the two input errors apart the way AWS lets it.
Fix: ops/create_cluster_v2.rs - emit the body-level message for the both-present case and keep it distinct from Cause 5.
Traces: kafka-create-cluster-v2-validation-errors#task#2.

Cause 7 of 8 - UpdateReplicationInfo resolves the replicator ARN before validating required source input.
AWS: UpdateReplicationInfo models BadRequestException (400) and NotFoundException (404). Input validation runs FIRST: a call that names neither sourceKafkaClusterArn nor sourceKafkaClusterId is refused 400 BadRequestException with `invalidParameter` 'sourceKafkaClusterArn' and `message` exactly "Must specify either sourceKafkaClusterArn or sourceKafkaClusterId for the source cluster." - even when the replicator ARN names nothing.
Emulator: 404 NotFoundException, invalidParameter 'replicatorArn', "Invalid parameter replicatorArn: The requested resource doesn't exist." - the lookup answers before the body is validated, so the input error is never reported.
Why: the caller is told the replicator is missing when its request is also malformed; it creates the replicator, retries, and gets a second, different failure. The gate order is the contract.
Fix: validate the request body before resolving ReplicatorArn in ops/update_replication_info.rs. (The open behavioral issue 20261029T175822Z-kafka-updatereplicationinfo-resolves-the-replicator-arn-before-validating-its.md describes this ordering from the behavioral suite and carries no wire fixture#step key.)
Traces: kafka-operate-on-nonexistent-replicator#task#2.

Cause 8 of 8 - the same inverted order hides a bad currentVersion.
AWS: UpdateReplicationInfo requires `currentVersion`. A value that is not a usable version is refused 400 BadRequestException with `message` exactly "Invalid parameter currentVersion." - all fixed text - again before the replicator lookup.
Emulator: 404 NotFoundException "Invalid parameter replicatorArn: The requested resource doesn't exist."
Why: as Cause 7; listed separately because it is a second required member whose validation is skipped, so fixing only the source-cluster check leaves this path diverging.
Fix: ops/update_replication_info.rs - validate currentVersion in the same pre-resolution pass.
Traces: kafka-replicator-crud-notfound-error-paths#task#3.

## Evidence
Corpus: simparity-wire-aws-v1. All eight clusters verdict=mismatch.

Cause 1 - kafka-cluster-create-unsupported-kafka-version-rejected#task#1, CreateClusterV2: status expected 400, got 200
  $.invalidParameter: expected 'kafkaVersion', got None
  $headers.x-amzn-errortype: expected a shaped error type, got None

Cause 2 - kafka-rejected-provisioning-requests#task#1, CreateCluster: status expected 400, got 200
  $.invalidParameter: expected 'brokerNodeGroupInfo', got None
  $.message: expected 'One or more of the specified client subnets don\u2019t exist.', got None
  $headers.x-amzn-errortype: expected 'BadRequestException', got None

Cause 3 - kafka-replicator-notfound-errors#task#4, CreateReplicator: status expected 400, got 200
  $.invalidParameter: expected 'subnetIds', got None
  $.message: expected 'Too many or too few subnet ids specified.', got None
  $headers.x-amzn-errortype: expected 'BadRequestException', got None
  (cluster also affects kafka-replicator-validation-errors#task#2 and #task#3)

Cause 4 - kafka-replicator-create-describe-delete-lifecycle#task#5, UpdateReplicationInfo: status expected 400, got 200
  $.message: expected 'Invalid parameter replicatorArn: This operation is only valid for resources that are in one of the following states: [RUNNING]', got None
  $headers.x-amzn-errortype: expected a shaped error type, got None

Cause 5 - kafka-create-cluster-v2-validation-errors#task#1, CreateClusterV2, 400 == 400:
  $.invalidParameter: expected 'clusterType', got 'Provisioned'
  $.message: expected 'You must specify a cluster type, either provisioned or serverless.', got 'A create names one cluster type: either Provisioned or Serverless must be given.'
  (cluster also affects kafka-rejected-provisioning-requests#task#2)

Cause 6 - kafka-create-cluster-v2-validation-errors#task#2, CreateClusterV2, 400 == 400:
  $.message: expected 'Invalid request body', got 'A create names one cluster type: Provisioned and Serverless cannot both be given.'

Cause 7 - kafka-operate-on-nonexistent-replicator#task#2, UpdateReplicationInfo: status expected 400, got 404
  $.invalidParameter: expected 'sourceKafkaClusterArn', got 'replicatorArn'
  $.message: expected 'Must specify either sourceKafkaClusterArn or sourceKafkaClusterId for the source cluster.', got 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.'
  $headers.x-amzn-errortype: expected 'BadRequestException', got 'NotFoundException'

Cause 8 - kafka-replicator-crud-notfound-error-paths#task#3, UpdateReplicationInfo: status expected 400, got 404
  $.message: expected 'Invalid parameter currentVersion.', got 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.'
  $headers.x-amzn-errortype: expected 'BadRequestException', got 'NotFoundException'

Smithy cited: kafka#CreateClusterV2 POST /api/v2/clusters, input ClusterName(required)/Provisioned/Serverless, errors BadRequestException(400)...; kafka#CreateCluster POST /v1/clusters, BrokerNodeGroupInfo and KafkaVersion required, errors BadRequestException(400)...; kafka#CreateReplicator POST /replication/v1/replicators, KafkaClusters/ReplicationInfoList/ReplicatorName/ServiceExecutionRoleArn required, errors BadRequestException(400)/NotFoundException(404); kafka#UpdateReplicationInfo PUT /replication/v1/replicators/{ReplicatorArn}/replication-info, CurrentVersion required, SourceKafkaClusterArn/SourceKafkaClusterId optional, errors BadRequestException(400)/NotFoundException(404).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: replay kafka-cluster-create-unsupported-kafka-version-rejected#task#1 - CreateClusterV2 with a provisioned.kafkaVersion the service does not list must answer 400 BadRequestException with invalidParameter 'kafkaVersion', and no cluster may be stored.
3. Cause 2: kafka-rejected-provisioning-requests#task#1 - CreateCluster with ClientSubnets that do not exist must answer 400, invalidParameter 'brokerNodeGroupInfo', message 'One or more of the specified client subnets don\u2019t exist.' (U+2019), with no cluster stored.
4. Cause 3: kafka-replicator-notfound-errors#task#4 - CreateReplicator with an out-of-range subnetIds list must answer 400, invalidParameter 'subnetIds', message 'Too many or too few subnet ids specified.', with no replicator stored.
5. Cause 4: kafka-replicator-create-describe-delete-lifecycle#task#5 - UpdateReplicationInfo on a replicator that is not in an update-accepting state must answer 400 with the "only valid for resources that are in one of the following states: [...]" message and must not apply the change.
6. Causes 5/6: kafka-create-cluster-v2-validation-errors#task#1 and #task#2 - CreateClusterV2 with NEITHER cluster type must answer invalidParameter 'clusterType' and 'You must specify a cluster type, either provisioned or serverless.'; with BOTH, message exactly 'Invalid request body'. Assert the two messages differ.
7. Cause 7: kafka-operate-on-nonexistent-replicator#task#2 - UpdateReplicationInfo on a nonexistent replicator ARN AND with no source cluster named must answer 400 BadRequestException, invalidParameter 'sourceKafkaClusterArn', the fixed source-cluster message - i.e. the body error, not the 404.
8. Cause 8: kafka-replicator-crud-notfound-error-paths#task#3 - the same call with an unusable currentVersion must answer 400 'Invalid parameter currentVersion.'
9. Re-replay all eight fixture#step bindings.
