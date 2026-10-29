Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.concurrent-topic-create-duplicate-race
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.concurrent-topic-create-duplicate-race|run#7|AssertionError|assert len(oks) == 2, f"expected both racing CreateTopic calls to be accepted, got {results}"
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# CreateTopic is not asynchronous: a second identical CreateTopic while the topic is still CREATING is refused with TopicExistsException instead of accepted

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTopic

## Description
REOPENS 20261022T171242Z-kafka-createtopic-stub.md: its fix left the call refused. CreateTopic now answers, but it refuses a call AWS accepts, so the operation is still unusable for the concurrent/retry pattern the check exercises.

Cause 1 of 1 - the name-uniqueness check runs at request time instead of once the topic exists.
AWS: Smithy kafka#CreateTopic (POST /v1/clusters/{ClusterArn}/topics, success code 200) models TopicExistsException (httpError 409) among its errors, and its output carries Status of type TopicState. Recorded behavior: CreateTopic is ASYNCHRONOUS. Two identical CreateTopic calls raced against the same cluster BOTH return HTTP 200, both with Status 'CREATING' and with the SAME TopicArn, and a third identical call issued while the topic is still CREATING is accepted too; ListTopics then shows that topic exactly ONCE. The name only becomes taken once the topic exists: a CreateTopic issued after the topic has reached ACTIVE is refused with Error.Code 'TopicExistsException'. So TopicExistsException is gated on the topic being settled, not on a record merely existing.
Emulator: the first racing call returns HTTP 200 with TopicArn and Status 'CREATING'; the second, concurrent, identical call is refused with Error.Code 'TopicExistsException'. The conflict fires as soon as the CREATING record is written, i.e. the state machine has no CREATING window in which the create is idempotent.
Why: this is the exact shape of every real caller of CreateTopic. A retrying SDK client, a parallel deploy, or two workers converging on the same topic list all issue the duplicate call inside the provisioning window; on AWS each of them gets 200 and the same TopicArn and proceeds, while against the emulator all but one get a 409 and treat it as a hard failure (TopicExistsException is not a retryable code). Code that is correct on AWS breaks only against the emulator - and only under concurrency, so it breaks intermittently.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/create_topic.rs together with the topic state machine it writes to: while a topic of that name on that cluster is in CREATING, an identical CreateTopic must be accepted and must answer 200 with the SAME TopicArn and Status 'CREATING' (idempotent re-entry), and the TopicExistsException conflict must be raised only once the topic has settled in ACTIVE. ListTopics must still report the topic once.
Traces: kafka.concurrent-topic-create-duplicate-race.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

kafka.concurrent-topic-create-duplicate-race (mismatch):
  two threads each call create_topic(ClusterArn=<the check's own cluster>, TopicName=<same name>, PartitionCount=1, ReplicationFactor=2)
  expected: BOTH calls return HTTP 200 with Status 'CREATING' and one shared TopicArn; then after the topic is ACTIVE, ListTopics shows it exactly once and a late identical create is refused ('error', 'TopicExistsException')
  actual   AssertionError: expected both racing CreateTopic calls to be accepted, got [('ok', {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'TopicArn': 'arn:aws:kafka:us-west-2:000000000000:topic/sim2real-topicrace-<rand>/<uuid>-1/sim2real-race-topic-<rand>', 'TopicName': 'sim2real-race-topic-<rand>', 'Status': 'CREATING'}), ('error', 'TopicExistsException')]

The winner's own response proves the emulator does model the asynchronous CREATING state (Status 'CREATING'); what is missing is that the second call inside that window is refused rather than returned the same handle.

## Steps to Reproduce
1. Boot the emulator and create a provisioned MSK cluster; wait for it to be usable.
2. From two threads simultaneously call kafka.create_topic(ClusterArn=<cluster>, TopicName='race-topic', PartitionCount=1, ReplicationFactor=2).
3. Expect BOTH responses to be HTTP 200 with Status 'CREATING' and an identical TopicArn. Observe one 200 and one 409 TopicExistsException.
4. Wait for the topic to reach ACTIVE, then kafka.list_topics(ClusterArn=<cluster>) -> expect exactly one entry named 'race-topic', whose TopicArn is the one both calls returned.
5. Issue the identical create_topic once more now that the topic is ACTIVE -> expect Error.Code 'TopicExistsException'.
6. Positive control: create_topic with a fresh non-colliding name -> expect 200 and the topic to reach ACTIVE.
7. Re-replay kafka.concurrent-topic-create-duplicate-race.
