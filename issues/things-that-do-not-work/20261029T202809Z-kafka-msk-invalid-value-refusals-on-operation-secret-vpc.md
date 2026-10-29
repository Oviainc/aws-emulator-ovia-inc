Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn kafka-scram-secret-error-paths kafka-scram-secret-malformed-cluster-arn kafka-scram-secret-nonexistent-and-malformed-cluster-errors kafka-vpc-connection-list-and-reject-on-foreign-cluster kafka-vpc-connection-management-workflow-malformed-arn kafka-list-configurations-error-paths kafka-replicator-notfound-errors
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DescribeClusterOperation|4xx->4xx|$.message
Issue-Key: kafka|DescribeClusterOperationV2|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DescribeClusterOperationV2|4xx->4xx|$.message
Issue-Key: kafka|ListScramSecrets|4xx->4xx|$.invalidParameter
Issue-Key: kafka|ListScramSecrets|4xx->4xx|$.message
Issue-Key: kafka|BatchAssociateScramSecret|4xx->4xx|$.invalidParameter
Issue-Key: kafka|BatchAssociateScramSecret|4xx->4xx|$.message
Issue-Key: kafka|BatchDisassociateScramSecret|4xx->4xx|$.invalidParameter
Issue-Key: kafka|BatchDisassociateScramSecret|4xx->4xx|$.message
Issue-Key: kafka|ListClientVpcConnections|4xx->4xx|$.invalidParameter
Issue-Key: kafka|ListClientVpcConnections|4xx->4xx|$.message
Issue-Key: kafka|ListConfigurations|4xx->4xx|$.message
Issue-Key: kafka|UpdateReplicationInfo|4xx->4xx|$.message
Issue-Key: kafka|DeleteReplicator|4xx->4xx|$.message
Issue-Related: 20261029T202809Z-kafka-msk-error-payloads-name-the-smithy-member-instead.md
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK invalid-value refusals on operation, secret, VPC-connection and pagination inputs carry emulator prose (9 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterOperation

## Description
NINE distinct root causes sharing one fault: the emulator answers 400 BadRequestException with the correct status and code but substitutes its own request-derived sentence for the fixed text MSK uses for that refusal class. Unlike the cluster-ARN bundle, these nine span THREE different fixed strings MSK chooses between, so a fixer must get the right one per path, not one blanket string. All nine must be closed.

AWS: every operation below models kafka#BadRequestException (httpError 400) with members `invalidParameter` (jsonName) and `message`. The three fixed strings recorded are:
  (a) "One or more of the parameters are not valid." - MSK's generic parameter refusal; all fixed text, nothing request-derived.
  (b) "The specified cluster ARN isn't valid. Specify a valid cluster ARN, and try again." (the apostrophe is U+2019) - MSK's cluster-ARN-specific refusal on the SCRAM and client-VPC-connection surfaces; all fixed text, the offending ARN is NOT echoed.
  (c) "The provided value is not valid." - MSK's refusal for a malformed replicator ARN; all fixed text.
In every case `invalidParameter` is the camelCase wire name of the parameter blamed.

Emulator: a self-composed sentence of the form "<offending value> is not a valid <resource description>." (or, for pagination, "The pagination token is not valid."), plus the PascalCase parameter token where one is asserted.

Why: status and code are right, so the divergence is invisible until a client string-matches MSK's text; and the emulator reflects the caller's input into messages AWS keeps constant, so the same request produces a different message than AWS for every distinct bad value. Choosing the wrong one of the three strings also mis-signals how specific the complaint is (generic parameter vs cluster ARN vs value).

Fix: give the service ONE refusal helper per class (generic-parameter, cluster-ARN, invalid-value) under services/service-com-amazonaws-kafka-kafka/src/ and have each handler below select the class MSK uses, rather than formatting prose locally.

Cause 1 of 9: DescribeClusterOperation with a malformed cluster-OPERATION ARN. AWS: 400, invalidParameter 'clusterOperationArn', message (a). Emulator: invalidParameter 'ClusterOperationArn', message "not-a-cluster-operation-arn is not a valid Amazon MSK cluster operation ARN." Fix: ops/describe_cluster_operation.rs.
Traces: kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#2.

Cause 2 of 9: DescribeClusterOperationV2, same input class. AWS: 400, invalidParameter 'clusterOperationArn', message (a). Emulator: 'ClusterOperationArn' and the same prose. Fix: ops/describe_cluster_operation_v2.rs.
Traces: kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#3.

Cause 3 of 9: ListScramSecrets with a malformed ClusterArn. AWS: 400, invalidParameter 'clusterArn', message (a). Emulator: 'ClusterArn' and "<value> is not a valid Amazon MSK cluster ARN." Fix: ops/list_scram_secrets.rs.
Traces: kafka-scram-secret-error-paths#task#3.

Cause 4 of 9: BatchAssociateScramSecret with a malformed ClusterArn. AWS uses the cluster-ARN-specific string here, NOT the generic one: 400, invalidParameter 'clusterArn', message (b). Emulator: 'ClusterArn' and its own prose. Note Causes 3 and 4 must NOT share one string - the same bad input yields (a) on ListScramSecrets and (b) on BatchAssociateScramSecret. Fix: ops/batch_associate_scram_secret.rs.
Traces: kafka-scram-secret-error-paths#task#1.

Cause 5 of 9: BatchDisassociateScramSecret with a malformed ClusterArn. AWS: 400, invalidParameter 'clusterArn', message (b). Emulator: 'ClusterArn' and its own prose. Fix: ops/batch_disassociate_scram_secret.rs.
Traces: kafka-scram-secret-error-paths#task#2.

Cause 6 of 9: ListClientVpcConnections with a malformed ClusterArn. AWS: 400, invalidParameter 'clusterArn', message (b). Emulator: 'ClusterArn' and its own prose. Fix: ops/list_client_vpc_connections.rs.
Traces: kafka-vpc-connection-list-and-reject-on-foreign-cluster#task#3.

Cause 7 of 9: ListConfigurations with an unusable `nextToken` query value. AWS: Smithy kafka#ListConfigurations binds NextToken to httpQuery 'nextToken' and models BadRequestException; the refusal message is (a) - MSK does NOT have a pagination-specific sentence. Emulator: "The pagination token is not valid." Fix: the nextToken decode path for ops/list_configurations.rs and the shared pagination-token helper it calls.
Traces: kafka-list-configurations-error-paths#task#1.

Cause 8 of 9: UpdateReplicationInfo with a malformed replicator ARN. AWS: 400, message (c) "The provided value is not valid." Emulator: "<value> is not a valid Amazon MSK replicator ARN." Fix: ops/update_replication_info.rs.
Traces: kafka-replicator-notfound-errors#task#2.

Cause 9 of 9: DeleteReplicator with a malformed replicator ARN. AWS: 400, message (c). Emulator: the same echoing prose. Fix: ops/delete_replicator.rs.
Traces: kafka-replicator-notfound-errors#task#3.

Neighbouring filings: 20261029T175822Z-kafka-msk-error-messages-are-the-emulator-s-own.md covers nine OTHER MSK message paths (duplicate names, unsupported KafkaVersions, MaxResults range, bootstrap-broker state) and 20261029T072939Z-kafka-scram-secret-refusals-no-cluster-state-gate.md covers the SCRAM cluster-state/SASL gates; neither pins the nine strings above and neither carries these fixture#step keys.

## Evidence
Corpus: simparity-wire-aws-v1. All nine clusters are verdict=mismatch with expected_status == actual_status == 400.

Cause 1 - kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#2, DescribeClusterOperation:
  $.invalidParameter: expected 'clusterOperationArn', got 'ClusterOperationArn'
  $.message: expected 'One or more of the parameters are not valid.', got 'not-a-cluster-operation-arn is not a valid Amazon MSK cluster operation ARN.'

Cause 2 - kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn#task#3, DescribeClusterOperationV2: identical expected/actual pair.

Cause 3 - kafka-scram-secret-error-paths#task#3, ListScramSecrets:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'
  $.message: expected 'One or more of the parameters are not valid.', got 'not-a-valid-cluster-arn is not a valid Amazon MSK cluster ARN.'
  (cluster also affects kafka-scram-secret-malformed-cluster-arn#task#3 and kafka-scram-secret-nonexistent-and-malformed-cluster-errors#task#1)

Cause 4 - kafka-scram-secret-error-paths#task#1, BatchAssociateScramSecret:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'
  $.message: expected 'The specified cluster ARN isn\u2019t valid. Specify a valid cluster ARN, and try again.', got 'not-a-valid-cluster-arn is not a valid Amazon MSK cluster ARN.'
  (cluster also affects kafka-scram-secret-malformed-cluster-arn#task#1 and kafka-scram-secret-nonexistent-and-malformed-cluster-errors#task#2)

Cause 5 - kafka-scram-secret-error-paths#task#2, BatchDisassociateScramSecret: same expected/actual as Cause 4 (cluster also affects kafka-scram-secret-malformed-cluster-arn#task#2).

Cause 6 - kafka-vpc-connection-list-and-reject-on-foreign-cluster#task#3, ListClientVpcConnections:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'
  $.message: expected 'The specified cluster ARN isn\u2019t valid. Specify a valid cluster ARN, and try again.', got 'not-a-valid-cluster-arn-sim2real-probe-kafka-ghostcluster-<suffix> is not a valid Amazon MSK cluster ARN.'
  (cluster also affects kafka-vpc-connection-management-workflow-malformed-arn#task#2)

Cause 7 - kafka-list-configurations-error-paths#task#1, ListConfigurations:
  $.message: expected 'One or more of the parameters are not valid.', got 'The pagination token is not valid.'

Cause 8 - kafka-replicator-notfound-errors#task#2, UpdateReplicationInfo:
  $.message: expected 'The provided value is not valid.', got 'not-a-valid-replicator-arn-sim2real-probe-kafka-ghost-<suffix> is not a valid Amazon MSK replicator ARN.'

Cause 9 - kafka-replicator-notfound-errors#task#3, DeleteReplicator: same expected/actual as Cause 8.

Smithy cited: kafka#BadRequestException (400) members invalidParameter/message; kafka#ListConfigurations NextToken -> httpQuery 'nextToken'; kafka#BatchAssociateScramSecret and kafka#BatchDisassociateScramSecret bind ClusterArn as an httpLabel and model both BadRequestException (400) and NotFoundException (404).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1/2: replay kafka-cluster-rolling-upgrade-and-operation-monitoring-malformed-arn steps task#2 and task#3 - DescribeClusterOperation / DescribeClusterOperationV2 with ClusterOperationArn='not-a-cluster-operation-arn' must answer 400 BadRequestException with invalidParameter 'clusterOperationArn' and message 'One or more of the parameters are not valid.'
3. Cause 3: kafka-scram-secret-error-paths#task#3 - ListScramSecrets with a malformed ClusterArn must use the GENERIC message.
4. Causes 4/5: kafka-scram-secret-error-paths#task#1 and #task#2 - the SAME malformed ClusterArn on BatchAssociateScramSecret / BatchDisassociateScramSecret must use the CLUSTER-ARN message 'The specified cluster ARN isn\u2019t valid. Specify a valid cluster ARN, and try again.' (U+2019 apostrophe). Assert steps 3 and 4 differ.
5. Cause 6: kafka-vpc-connection-list-and-reject-on-foreign-cluster#task#3 - ListClientVpcConnections, same cluster-ARN message, with the offending ARN absent from the text.
6. Cause 7: kafka-list-configurations-error-paths#task#1 - ListConfigurations?nextToken=<unusable> must answer the generic parameter message, not a pagination-specific one.
7. Causes 8/9: kafka-replicator-notfound-errors#task#2 and #task#3 - UpdateReplicationInfo / DeleteReplicator with a malformed replicator ARN must answer message 'The provided value is not valid.'
8. Re-replay all nine fixture#step bindings.
