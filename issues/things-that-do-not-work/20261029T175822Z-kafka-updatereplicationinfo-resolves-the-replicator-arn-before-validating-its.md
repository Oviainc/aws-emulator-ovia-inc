Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.update-replication-info-nonexistent-replicator-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.update-replication-info-nonexistent-replicator-rejected|run#10|AssertionError|assert code == expected_update, f"expected {expected_update!r}, got {code!r}"
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# UpdateReplicationInfo resolves the replicator ARN before validating its required source/target input, so a missing source cluster is reported as NotFoundException

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateReplicationInfo

## Description
Cause 1 of 1 - the gate order is inverted: resource lookup runs ahead of request-body validation.
AWS: Smithy kafka#UpdateReplicationInfo (PUT /replication/v1/replicators/{ReplicatorArn}/replication-info) takes ReplicatorArn as a required httpLabel and CurrentVersion as @required, while SourceKafkaClusterArn, SourceKafkaClusterId, TargetKafkaClusterArn and TargetKafkaClusterId are each individually optional - the model cannot express that exactly one of each pair must be supplied, so that rule is enforced at runtime. It models BadRequestException (httpError 400) and NotFoundException (404). Recorded behavior: the body rule is checked FIRST. A request that names neither SourceKafkaClusterArn nor SourceKafkaClusterId is refused HTTP 400 / Error.Code 'BadRequestException' with Error.Message exactly "Must specify either sourceKafkaClusterArn or sourceKafkaClusterId for the source cluster." - all fixed text, naming the two wire member names (jsonName 'sourceKafkaClusterArn' / 'sourceKafkaClusterId'), and it is returned even when the ReplicatorArn in the path names a replicator that does not exist. Only a request whose body is complete proceeds to replicator resolution and can come back NotFoundException.
Emulator: it resolves the path ReplicatorArn first and answers HTTP 404 / Error.Code 'NotFoundException' with its own message "Invalid parameter replicatorArn: The requested resource doesn\u2019t exist." - so the body defect is never reported, and the message is a hybrid the service does not emit (MSK's fixed not-found text is "The requested resource doesn\u2019t exist." with no 'Invalid parameter <member>:' prefix).
Why: a client that sends an incomplete UpdateReplicationInfo is told its replicator ARN is wrong. It then goes and re-resolves or re-creates the replicator - the wrong remedy entirely - instead of adding the source-cluster member it omitted, and the error never names the member at fault. Gate ordering is observable contract here precisely because the two failures carry different statuses (400 vs 404) and different exception types.
Fix: in services/service-com-amazonaws-kafka-kafka/src/ops/update_replication_info.rs, validate the request body before resolving ReplicatorArn - enforce 'exactly one of sourceKafkaClusterArn/sourceKafkaClusterId' and 'exactly one of targetKafkaClusterArn/targetKafkaClusterId' with the fixed BadRequestException messages naming those wire members, then resolve the replicator and refuse an unknown one with the service's plain NotFoundException text (drop the 'Invalid parameter replicatorArn:' prefix). A fixed issue, 20261025T025102Z-kafka-updatereplicationinfo-is-an-unimplemented-scaffold-stub-answers-501.md, stood the operation up from a 501; it did not establish this ordering.
Traces: kafka.update-replication-info-nonexistent-replicator-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

kafka.update-replication-info-nonexistent-replicator-rejected (mismatch):
  update_replication_info(ReplicatorArn=<well-formed, never-issued replicator ARN>, CurrentVersion=<any>, with no SourceKafkaClusterArn and no SourceKafkaClusterId)
  expected ('BadRequestException', 'Must specify either sourceKafkaClusterArn or sourceKafkaClusterId for the source cluster.')  [HTTP 400]
  actual   AssertionError: expected ('BadRequestException', 'Must specify either sourceKafkaClusterArn or sourceKafkaClusterId for the source cluster.'), got ('NotFoundException', 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.')

Two things diverge together and have one cause: the status/code (400 BadRequestException vs 404 NotFoundException, because the wrong gate fired) and the message (MSK's fixed body-validation sentence vs an invented 'Invalid parameter replicatorArn:' prefix on the not-found text).

## Steps to Reproduce
1. Boot the emulator.
2. kafka.update_replication_info(ReplicatorArn='arn:aws:kafka:<region>:<account>:replicator/never-issued/00000000-0000-0000-0000-000000000000-1', CurrentVersion='1', TargetKafkaClusterArn='arn:aws:kafka:<region>:<account>:cluster/x/00000000-0000-0000-0000-000000000000-1') - i.e. omit BOTH SourceKafkaClusterArn and SourceKafkaClusterId -> expect HTTP 400, Error.Code 'BadRequestException', Error.Message 'Must specify either sourceKafkaClusterArn or sourceKafkaClusterId for the source cluster.' Observe HTTP 404 'NotFoundException' with 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.'
3. Repeat with a COMPLETE body (both a source and a target member supplied) against the same never-issued ReplicatorArn -> expect HTTP 404 'NotFoundException' with Message exactly 'The requested resource doesn\u2019t exist.' (no 'Invalid parameter ...' prefix).
4. Re-replay kafka.update-replication-info-nonexistent-replicator-rejected.
