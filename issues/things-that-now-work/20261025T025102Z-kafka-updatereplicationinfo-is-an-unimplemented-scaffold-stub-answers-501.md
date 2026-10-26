Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.update-replication-info-invalid-target-topics-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.update-replication-info-invalid-target-topics-rejected|_wait_cluster_v2_active#4|Timeout|time.sleep(interval_s)
Issue-Commit: 19ca13517733601aaaac557341193877c880d409

# UpdateReplicationInfo is an unimplemented scaffold stub: answers 501 NotImplemented where AWS updates the replication info or refuses a mismatched source/target pair with BadRequestException

> _Note: this issue was identified against the code base at commit `19ca13517733601aaaac557341193877c880d409`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateReplicationInfo

## Description
Single cause (Cause 1 of 1): the kafka UpdateReplicationInfo handler is still the generated scaffold stub, so neither the accepting path nor the modeled rejection path of the operation exists.

AWS: Smithy (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json) binds UpdateReplicationInfo to PUT /replication/v1/replicators/{ReplicatorArn}/replication-info, success 200, input {ReplicatorArn (httpLabel, required), CurrentVersion (required), SourceKafkaClusterArn, TargetKafkaClusterArn, SourceKafkaClusterId, TargetKafkaClusterId, TopicReplication (TopicReplicationUpdate), ConsumerGroupReplication, LogDelivery}, output {ReplicatorArn, ReplicatorState (enum CREATING|DELETING|FAILED|RUNNING|UPDATING)}, errors BadRequestException (400), ForbiddenException (403), NotFoundException (404), UnauthorizedException (401), TooManyRequestsException (429), InternalServerErrorException (500), ServiceUnavailableException (503). Two behaviors the model therefore requires: a well-formed update against a configured source->target pair is ACCEPTED (200, ReplicatorState UPDATING then RUNNING, and the new TopicReplication visible on a later DescribeReplicator, with CurrentVersion bumped), and an update whose source/target pair matches no configured replication-info entry is REFUSED with HTTP 400 / error code `BadRequestException` (recorded example message, not a requirement: "The replication info specified with the kafka cluster aliases does not exist."), leaving the stored replication-info orientation, TopicReplication and CurrentVersion untouched.

Emulator: services/service-com-amazonaws-kafka-kafka/src/ops/update_replication_info.rs line 174 returns crate::scaffolded::Scaffolded::not_implemented("UpdateReplicationInfo") — HTTP 501 with error code `NotImplemented` and message "operation UpdateReplicationInfo is scaffolded but not implemented in this emulator" — for every request. 501/NotImplemented is not a modeled error of this operation, so neither the accept nor the 400 refusal can ever be observed. (Hypothesis: the check that exercises this is reported as a 60s harness timeout rather than a 501 because it spends its budget in its own 30s-interval waits for the two Serverless clusters it needs before it ever reaches UpdateReplicationInfo; the stub above is read directly from the checkout and is independent of that.)

Why: a client cannot change a replicator's TopicReplication/ConsumerGroupReplication configuration at all, and code that handles the modeled BadRequestException for a mismatched cluster pair instead sees an unmodeled 501 — which SDK retry policies treat as a non-retryable server fault and error handlers do not match, so the request surfaces as an emulator fault rather than the client error AWS returns.

Fix: implement the handler in services/service-com-amazonaws-kafka-kafka/src/ops/update_replication_info.rs against the replicator registry in src/state.rs: resolve the replicator through the settled lookup (NotFoundException for a well-formed ARN this region holds none for, BadRequestException for a value not shaped like a replicator ARN), enforce the CurrentVersion optimistic-concurrency check, require the supplied source/target pair to match a stored ReplicationInfo entry (BadRequestException 400 when it does not, with NO mutation of the stored entry or CurrentVersion), and on acceptance fold the TopicReplication/ConsumerGroupReplication update into the record and bump CurrentVersion, reporting ReplicatorState through the same settling path DescribeReplicator uses.

Traces: services/service-com-amazonaws-kafka-kafka/src/ops/update_replication_info.rs lines 157-175 (stub handler); replicator record and settling at services/service-com-amazonaws-kafka-kafka/src/state.rs (Replicator, REPLICATOR_ACTIVATION_DELAY, settled_replicator_in). Failing check: kafka.update-replication-info-invalid-target-topics-rejected.

## Evidence
Check kafka.update-replication-info-invalid-target-topics-rejected [mismatch]:
  Error: Check timed out after 60.0s

The check creates two Serverless clusters and a replicator, waits for RUNNING, then (a) issues a correct-pair UpdateReplicationInfo as a positive control and asserts ReplicatorState in ("UPDATING","RUNNING") and that DetectAndCopyNewTopics=True becomes visible on an independent DescribeReplicator, and (b) issues a swapped source/target UpdateReplicationInfo and asserts (Error.Code, Error.Message) == ("BadRequestException", "The replication info specified with the kafka cluster aliases does not exist.") with CurrentVersion and the stored orientation unchanged.

Neither assertion can hold against this emulator, from the checkout itself:
  services/service-com-amazonaws-kafka-kafka/src/ops/update_replication_info.rs
    157:    async fn handle(&self, input, ctx) -> Result<Self::Output, Self::Error> {
    164:        let _ = (&input, ctx, &self.engine);
    174:        Err(crate::scaffolded::Scaffolded::not_implemented("UpdateReplicationInfo"))
That channel serializes as HTTP 501, error code `NotImplemented`, message "operation UpdateReplicationInfo is scaffolded but not implemented in this emulator" (the same shape the other scaffolded kafka operations in this suite returned, e.g. BatchAssociateScramSecret, ListNodes, DeleteConfiguration).

Expected from the model: 200 with {ReplicatorArn, ReplicatorState} for an accepted update; HTTP 400 with error code BadRequestException for a source/target pair that matches no configured replication-info entry.
Actual: HTTP 501, error code NotImplemented — not a modeled error of UpdateReplicationInfo.

Note: no open issue covers this operation (the open kafka stub issues are for RebootBroker, DeleteConfiguration, DescribeConfigurationRevision, UpdateClusterKafkaVersion, BatchAssociateScramSecret and ListNodes), and the suite did not mark this check ALREADY FILED because it timed out before issuing the call.

## Steps to Reproduce
1. Build and boot the emulator; point a botocore client at it (service `kafka`).
2. Create two Serverless clusters: kafka.create_cluster_v2(ClusterName=<unique>, Serverless={VpcConfigs: [{SubnetIds: [<two subnets in distinct AZs>], SecurityGroupIds: [<sg>]}], ClientAuthentication: {Sasl: {Iam: {Enabled: true}}}}) and wait for State ACTIVE on DescribeClusterV2 (settles in well under a second here).
3. Create an IAM role trusted by kafka.amazonaws.com and call kafka.create_replicator(ReplicatorName=<unique>, ServiceExecutionRoleArn=<role>, KafkaClusters=[{AmazonMskCluster: {MskClusterArn: <source>}, VpcConfig: {...}}, {AmazonMskCluster: {MskClusterArn: <target>}, VpcConfig: {...}}], ReplicationInfoList=[{SourceKafkaClusterArn: <source>, TargetKafkaClusterArn: <target>, TargetCompressionType: "NONE", TopicReplication: {...}, ConsumerGroupReplication: {...}}]); wait for ReplicatorState RUNNING via DescribeReplicator.
4. Positive path: kafka.update_replication_info(ReplicatorArn=<arn>, CurrentVersion=<current>, SourceKafkaClusterArn=<source>, TargetKafkaClusterArn=<target>, TopicReplication={TopicsToReplicate: ["t-.*"], TopicsToExclude: [], CopyTopicConfigurations: true, CopyAccessControlListsForTopics: false, DetectAndCopyNewTopics: true}).
   Observed: HTTP 501 NotImplemented. Expected: HTTP 200 with ReplicatorState UPDATING/RUNNING and the change visible on DescribeReplicator.
5. Negative path: repeat step 4 with SourceKafkaClusterArn/TargetKafkaClusterArn swapped.
   Observed: HTTP 501 NotImplemented. Expected: HTTP 400 with Error.Code "BadRequestException" and no change to the stored replication info or CurrentVersion.
