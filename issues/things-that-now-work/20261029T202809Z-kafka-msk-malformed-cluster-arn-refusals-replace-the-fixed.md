Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-broker-inventory-and-reboot-malformed-arn
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-broker-inventory-and-reboot-malformed-arn kafka-list-nodes-error-paths kafka-cluster-capacity-scaling-malformed-arn kafka-cluster-decommission-malformed-arn kafka-cluster-hardening-and-observability-malformed-arn kafka-cluster-policy-error-paths kafka-cluster-policy-management-malformed-arn kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|ListNodes|4xx->4xx|$.invalidParameter
Issue-Key: kafka|ListNodes|4xx->4xx|$.message
Issue-Key: kafka|UpdateBrokerStorage|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateBrokerStorage|4xx->4xx|$.message
Issue-Key: kafka|UpdateBrokerType|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateBrokerType|4xx->4xx|$.message
Issue-Key: kafka|DeleteCluster|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DeleteCluster|4xx->4xx|$.message
Issue-Key: kafka|UpdateSecurity|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateSecurity|4xx->4xx|$.message
Issue-Key: kafka|UpdateStorage|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateStorage|4xx->4xx|$.message
Issue-Key: kafka|GetClusterPolicy|4xx->4xx|$.invalidParameter
Issue-Key: kafka|GetClusterPolicy|4xx->4xx|$.message
Issue-Key: kafka|DeleteClusterPolicy|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DeleteClusterPolicy|4xx->4xx|$.message
Issue-Key: kafka|UpdateClusterKafkaVersion|4xx->4xx|$.message
Issue-Key: kafka|ListClusterOperations|4xx->4xx|$.invalidParameter
Issue-Key: kafka|ListClusterOperations|4xx->4xx|$.message
Issue-Related: 20261029T202809Z-kafka-msk-error-payloads-name-the-smithy-member-instead.md
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK malformed cluster-ARN refusals replace the fixed parameter message with emulator prose (10 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNodes

## Description
TEN distinct root causes sharing one fault: for a `ClusterArn` that is not shaped like an MSK cluster ARN, the emulator returns the right HTTP status (400) and the right error code (BadRequestException) but writes its own request-derived sentence into `message` instead of MSK's fixed generic parameter message, and names the parameter in PascalCase. Each operation is a separate refusal path in the crate, so each is its own cause; all ten must be closed.

AWS (common to all ten): the cluster-scoped MSK operations below all model kafka#BadRequestException (httpError 400), whose members are `invalidParameter` and `message`. For a malformed cluster ARN the recorded response is 400 / x-amzn-errortype BadRequestException with `message` exactly "One or more of the parameters are not valid." - the WHOLE string is FIXED text: no part of it is request-derived, and in particular the offending ARN is NOT echoed. `invalidParameter` is the modeled wire name `clusterArn`.

Emulator (common to all ten): 400 / BadRequestException with `invalidParameter` = 'ClusterArn' and `message` = "<the offending ARN string> is not a valid Amazon MSK cluster ARN." - a self-composed sentence that both diverges from AWS's wording and leaks the caller's input into a message AWS keeps constant.

Why (common to all ten): the status and code are correct, so a client that classifies or greps MSK refusals by their message silently stops matching, and an operator reading the text is shown a sentence AWS never emits. Echoing the request value also makes the message non-constant across runs where AWS's is constant.

Fix (common to all ten): the malformed-cluster-ARN rejection must come from ONE shared helper for the service (under services/service-com-amazonaws-kafka-kafka/src/), emitting the fixed string and the camelCase parameter token, and each listed operation's handler must call it instead of formatting its own sentence. The per-operation handlers below are where the prose is currently composed.

Cause 1 of 10: ListNodes (GET /v1/clusters/{ClusterArn}/nodes) - handler services/service-com-amazonaws-kafka-kafka/src/ops/list_nodes.rs.
Traces: kafka-broker-inventory-and-reboot-malformed-arn#task#1 (cluster also covers kafka-list-nodes-error-paths#task#1).

Cause 2 of 10: UpdateBrokerStorage - ops/update_broker_storage.rs.
Traces: kafka-cluster-capacity-scaling-malformed-arn#task#2.

Cause 3 of 10: UpdateBrokerType - ops/update_broker_type.rs.
Traces: kafka-cluster-capacity-scaling-malformed-arn#task#3.

Cause 4 of 10: DeleteCluster - ops/delete_cluster.rs.
Traces: kafka-cluster-decommission-malformed-arn#task#1 (cluster also covers #task#2).

Cause 5 of 10: UpdateSecurity - ops/update_security.rs.
Traces: kafka-cluster-hardening-and-observability-malformed-arn#task#1.

Cause 6 of 10: UpdateStorage - ops/update_storage.rs.
Traces: kafka-cluster-hardening-and-observability-malformed-arn#task#4.

Cause 7 of 10: GetClusterPolicy - ops/get_cluster_policy.rs.
Traces: kafka-cluster-policy-error-paths#task#1 (cluster also covers kafka-cluster-policy-management-malformed-arn#task#1).

Cause 8 of 10: DeleteClusterPolicy - ops/delete_cluster_policy.rs.
Traces: kafka-cluster-policy-error-paths#task#2 (cluster also covers kafka-cluster-policy-management-malformed-arn#task#2).

Cause 9 of 10: UpdateClusterKafkaVersion - ops/update_cluster_kafka_version.rs.
Traces: kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#1.

Cause 10 of 10: ListClusterOperations - ops/list_cluster_operations.rs.
Traces: kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#4.

Neighbouring filing: the open behavioral issue 20261029T175822Z-kafka-msk-error-messages-are-the-emulator-s-own.md documents the same fixed string "One or more of the parameters are not valid." for ListConfigurations MaxResults (its Cause 8). It does not carry any of the fixture#step keys above and none of these ten operations appear in its trace ids, so the malformed-cluster-ARN paths are not covered by it.

## Evidence
Corpus: simparity-wire-aws-v1. All ten clusters are verdict=mismatch with expected_status == actual_status == 400; the diverging body keys are $.invalidParameter and $.message only.

Common divergence, repeated per operation:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'
  $.message: expected 'One or more of the parameters are not valid.'
           got 'not-a-cluster-arn is not a valid Amazon MSK cluster ARN.'
  (for Causes 7 and 8 the echoed value is the fixture's longer malformed string, 'not-a-valid-cluster-arn-sim2real-probe-kafka-ghostcluster-<suffix>', confirming the message is request-derived where AWS's is constant)

Per cause:
  Cause 1  ListNodes                 - kafka-broker-inventory-and-reboot-malformed-arn#task#1
  Cause 2  UpdateBrokerStorage       - kafka-cluster-capacity-scaling-malformed-arn#task#2
  Cause 3  UpdateBrokerType          - kafka-cluster-capacity-scaling-malformed-arn#task#3
  Cause 4  DeleteCluster             - kafka-cluster-decommission-malformed-arn#task#1
  Cause 5  UpdateSecurity            - kafka-cluster-hardening-and-observability-malformed-arn#task#1
  Cause 6  UpdateStorage             - kafka-cluster-hardening-and-observability-malformed-arn#task#4
  Cause 7  GetClusterPolicy          - kafka-cluster-policy-error-paths#task#1
  Cause 8  DeleteClusterPolicy       - kafka-cluster-policy-error-paths#task#2
  Cause 9  UpdateClusterKafkaVersion - kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#1
  Cause 10 ListClusterOperations     - kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#4

Smithy cited: kafka#BadRequestException httpError 400, members InvalidParameter (jsonName 'invalidParameter') and Message (jsonName 'message'); the cluster-scoped operations bind ClusterArn as an httpLabel.

Fixed vs variable in the required message: the entire sentence "One or more of the parameters are not valid." is fixed text with NO variable part; the only request-derived field in the payload is invalidParameter, whose value is the parameter NAME ('clusterArn'), not its value.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture kafka-broker-inventory-and-reboot-malformed-arn step task#1: ListNodes with ClusterArn='not-a-cluster-arn' must answer HTTP 400, x-amzn-errortype BadRequestException, body {"invalidParameter":"clusterArn","message":"One or more of the parameters are not valid."} with the offending ARN absent from the message.
3. Repeat for each fixture#step in trace_ids (UpdateBrokerStorage, UpdateBrokerType, DeleteCluster, UpdateSecurity, UpdateStorage, GetClusterPolicy, DeleteClusterPolicy, UpdateClusterKafkaVersion, ListClusterOperations) and assert the identical payload.
4. Confirm the message is constant: call two of them with different malformed ARN strings and assert byte-identical message fields.
5. Re-replay all ten fixture#step bindings.
