Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.deploy-msk-serverless-cfn-topic-createable
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateTopic|BadRequestException
Issue-Commit: ee5f79e932caffa01c897fc87f80cd65e8d793a6

# Topic Management APIs are not gated on cluster type: CreateTopic/DescribeTopic against an MSK Serverless cluster fall through to broker-count validation instead of ServerlessUnsupportedException

> _Note: this issue was identified against the code base at commit `ee5f79e932caffa01c897fc87f80cd65e8d793a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTopic

## Description
Cause 1 of 1: the MSK Topic Management API operations (CreateTopic, DescribeTopic, and the sibling topic operations that share the same cluster-resolution path) never look at the resolved cluster's type, so a SERVERLESS cluster is treated as a provisioned cluster with zero brokers.

AWS: MSK Serverless does not expose the AWS-API Topic Management surface at all. Topics on a serverless cluster are created and read only over the native Kafka protocol, so real AWS refuses every Topic Management call against a cluster whose ClusterType is SERVERLESS deterministically and immediately, with error code `ServerlessUnsupportedException` and the message `Topic APIs are not supported on serverless clusters.` This refusal is recorded behavior only: the on-disk Smithy model (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json) declares no `ServerlessUnsupportedException` shape and CreateTopic's modeled error list is BadRequestException/ClusterConnectivityException/ConflictException/ControllerMovedException/ForbiddenException/GroupSubscribedToTopicException/InternalServerErrorException/KafkaRequestException/KafkaTimeoutException/NotControllerException/ReassignmentInProgressException/ServiceUnavailableException/TooManyRequestsException/TopicExistsException/UnauthorizedException/UnknownTopicOrPartitionException — the serverless refusal is an undeclared-but-recorded error, so the emulator has to widen the shape in the way it already widens NotFoundException into CreateTopic. In the refusal message the whole string is FIXED text; it carries no value from the request or from state (no cluster ARN, no topic name, no limit).

Emulator: CreateTopic on a CloudFormation-deployed AWS::MSK::ServerlessCluster that has reached ACTIVE with ClusterType=SERVERLESS resolves the cluster, skips any cluster-type check, and reaches the replication-factor bound, which compares the request's ReplicationFactor against the cluster's stored broker count. A serverless cluster has no broker nodes, so that count is 0 and every call is refused as HTTP 400 `BadRequestException` with invalidParameter=`ReplicationFactor` and the message template `Replication factor: <ReplicationFactor> larger than available brokers: <brokerCount>.` (observed in the replay as `Replication factor: 3 larger than available brokers: 0.` — the numbers there are this run's request value and the cluster's stored broker count, not part of the requirement). The correct answer never depends on ReplicationFactor at all: even ReplicationFactor=1 must be refused as serverless-unsupported, and with ReplicationFactor=1 the emulator would instead create the topic and report success, which is the more damaging half of the same defect. DescribeTopic on the same cluster sits on the same ungated path and answers the cluster/topic not-found 404 rather than the serverless refusal.

Why: a client cannot distinguish "this API does not exist for serverless" from "your replication factor is too high". Provisioning code that retries with a lower replication factor gets a fabricated success and a topic the emulator registers but that no serverless cluster could ever hold, so Kafka-protocol clients later find no such topic; code that branches on ServerlessUnsupportedException to fall back to the native Kafka admin path never takes that branch and fails on a parameter error it cannot fix. Error-handling tests that assert the serverless refusal see a wrong code, a wrong invalidParameter and a wrong message.

Fix: the cluster-type gate belongs in the Topic Management handlers' validation, immediately after the cluster is resolved and BEFORE the replication-factor/broker-count bound and before any topic registration — in CreateTopic's handler and in the same position in DescribeTopic and its topic siblings, with the serverless error shape widened into those operations' error channels the way NotFoundException already is.

Traces: kafka.deploy-msk-serverless-cfn-topic-createable (the check creates the serverless cluster through CloudFormation, waits for ACTIVE/SERVERLESS, then asserts both CreateTopic and DescribeTopic are refused with ServerlessUnsupportedException); handler and the diverging branch at services/service-com-amazonaws-kafka-kafka/src/ops/create_topic.rs lines 247-271.

## Evidence
Check: kafka.deploy-msk-serverless-cfn-topic-createable [mismatch]

Expected (real AWS, recorded): CreateTopic against a cluster with ClusterType=SERVERLESS is refused with error code `ServerlessUnsupportedException` and message `Topic APIs are not supported on serverless clusters.` (entirely fixed text). DescribeTopic against the same cluster is refused identically.

Actual (emulator):
  AssertionError: expected CreateTopic on a serverless cluster to be refused with ServerlessUnsupportedException, got 'BadRequestException': An error occurred (BadRequestException) when calling the CreateTopic operation: Replication factor: 3 larger than available brokers: 0.

HTTP 400 / BadRequestException with invalidParameter="ReplicationFactor", instead of the serverless refusal. The positive controls in the same check passed: the CFN stack reached CREATE_COMPLETE and DescribeClusterV2 reported State=ACTIVE, ClusterType=SERVERLESS, so the cluster was correctly serverless at the moment of the call.

Source: services/service-com-amazonaws-kafka-kafka/src/ops/create_topic.rs — the handler resolves the cluster (line 247), then applies `input.replication_factor > cluster.number_of_broker_nodes` (line 260) with no check of the cluster's type anywhere in the handler.

Smithy: CreateTopic POST /v1/clusters/{ClusterArn}/topics, success 200; no ServerlessUnsupportedException shape exists in the model (smithy_query shape=ServerlessUnsupportedException -> not found), so this refusal is recorded behavior only and must be widened in like NotFoundException already is at create_topic.rs lines 130-133.

## Steps to Reproduce
1. Deploy a CloudFormation stack containing an AWS::MSK::ServerlessCluster (two subnets in distinct AZs, one security group, SASL/IAM client authentication) and wait for CREATE_COMPLETE.
2. kafka DescribeClusterV2 on the stack's cluster ARN and wait for State=ACTIVE; confirm ClusterType=SERVERLESS.
3. Call kafka CreateTopic(ClusterArn=<that serverless cluster ARN>, TopicName=<any valid topic name>, PartitionCount=1, ReplicationFactor=3).
   Expected: refusal with code ServerlessUnsupportedException, message "Topic APIs are not supported on serverless clusters."
   Actual: HTTP 400 BadRequestException, invalidParameter=ReplicationFactor, message "Replication factor: 3 larger than available brokers: 0."
4. Repeat step 3 with ReplicationFactor=1 to see the other half of the same defect: the emulator creates the topic and returns 200 where AWS still refuses with ServerlessUnsupportedException.
5. Call kafka DescribeTopic(ClusterArn=<that ARN>, TopicName=<the same name>): expected the same ServerlessUnsupportedException refusal; the emulator answers its cluster/topic not-found 404 instead.
