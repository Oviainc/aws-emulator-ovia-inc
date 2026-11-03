Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.topic-delete-nonexistent-not-found kafka.topic-create-invalid-replication-factor-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.topic-delete-nonexistent-not-found|run#5|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: kafka.topic-create-invalid-replication-factor-rejected|run#22|AssertionError|assert again == expected_again, (
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# DeleteTopic refuses a topic the cluster does not hold with UnknownTopicOrPartitionException instead of NotFoundException

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTopic

## Description
Cause 1 of 1 - the wrong one of two same-status modeled errors is selected for a topic lookup miss.
AWS: Smithy kafka#DeleteTopic (DELETE /v1/clusters/{ClusterArn}/topics/{TopicName}) models BOTH NotFoundException (httpError 404) and UnknownTopicOrPartitionException (httpError 404). Recorded behavior: deleting a topic the cluster does not hold - whether it never existed, or was already deleted by a previous DeleteTopic - is refused with Error.Code 'NotFoundException' and Error.Message "The requested resource doesn\u2019t exist." (all fixed text; the apostrophe is U+2019, not ASCII, and neither the topic name nor the cluster ARN is echoed). UnknownTopicOrPartitionException exists in the model but is not the code MSK's control plane returns for this class; the two are distinguishable only by Error.Code, since both are 404 and the emulator already emits the correct Message.
Emulator: HTTP 404 with the correct Message "The requested resource doesn\u2019t exist." but Error.Code 'UnknownTopicOrPartitionException'. The same substitution happens on the first delete of a never-created topic and on a repeat delete of an already-deleted topic, so it is one selection defect and not two.
Why: Error.Code is what botocore and every other SDK use to build the exception type a client catches. A client that catches the modeled kafka NotFoundException (or that dispatches on the string 'NotFoundException' to treat the delete as already-done and continue) sees an unrelated exception type and fails a teardown that AWS lets it ignore. Because the status and the message are already right, nothing else in the response hints at the mismatch.
Fix: the topic lookup-miss refusal in services/service-com-amazonaws-kafka-kafka/src/ops/delete_topic.rs - return the service's NotFoundException for a topic that is not present on the cluster, keeping the fixed message, and reserve UnknownTopicOrPartitionException for the broker-protocol condition it models.
Traces: kafka.topic-delete-nonexistent-not-found, kafka.topic-create-invalid-replication-factor-rejected (the latter reaches the same refusal on its second, repeat delete).

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

kafka.topic-delete-nonexistent-not-found (mismatch):
  delete_topic(ClusterArn=<a real cluster>, TopicName=<never created>)
  expected Error.Code 'NotFoundException'
  actual   AssertionError: expected NotFoundException, got UnknownTopicOrPartitionException

kafka.topic-create-invalid-replication-factor-rejected (mismatch) - the repeat-delete leg of the same refusal:
  delete_topic(<topic>) succeeds, then delete_topic(<the same, now-deleted topic>)
  expected ('NotFoundException', 'The requested resource doesn\u2019t exist.')
  actual   AssertionError: a second delete of the already-deleted topic must also fail: expected ('NotFoundException', 'The requested resource doesn\u2019t exist.'), got ('UnknownTopicOrPartitionException', 'The requested resource doesn\u2019t exist.')

Note the Message is byte-identical in both legs - only Error.Code diverges, which is why this is a single code-selection defect.

## Steps to Reproduce
1. Boot the emulator and create a provisioned MSK cluster.
2. kafka.delete_topic(ClusterArn=<cluster>, TopicName='never-created') -> expect HTTP 404, Error.Code 'NotFoundException', Error.Message 'The requested resource doesn\u2019t exist.' (U+2019). Observe Error.Code 'UnknownTopicOrPartitionException'.
3. kafka.create_topic(ClusterArn=<cluster>, TopicName='t1', PartitionCount=1, ReplicationFactor=2); wait for ACTIVE; kafka.delete_topic(ClusterArn=<cluster>, TopicName='t1') -> expect success; call delete_topic for 't1' again -> expect the same 404 'NotFoundException' with the same fixed message.
4. Re-replay kafka.topic-delete-nonexistent-not-found and kafka.topic-create-invalid-replication-factor-rejected.
