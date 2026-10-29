Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.create-with-invalid-input-should-fail
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.create-with-invalid-input-should-fail|_assert_rejected#10|AssertionError|assert code is not None, (
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# CreateCluster accepts BrokerNodeGroupInfo.ClientSubnets that do not exist and mints a cluster

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1 - CreateCluster does not resolve its broker subnets before creating the cluster.
AWS: Smithy kafka#CreateCluster (POST /v1/clusters) requires BrokerNodeGroupInfo (@required), whose ClientSubnets name the subnets the brokers are placed in, and models BadRequestException (httpError 400), ConflictException (409), ForbiddenException (403), UnauthorizedException (401) and ValidationException-class client faults. Recorded behavior: a CreateCluster whose ClientSubnets name subnet ids that do not exist in the account/region is REFUSED before any cluster is created - with one of the client-fault codes MSK uses for an unresolvable target (AccessDeniedException / BadRequestException / ForbiddenException / NotFoundException / UnauthorizedException / ValidationException; which one depends on whether the caller's permissions are resource-scoped, because MSK authorizes before it resolves, so the check accepts any of them). The requirement is that the call does NOT return 200 and does NOT return a resource identifier, and that no cluster under that name appears in ListClustersV2 afterwards.
Emulator: HTTP 200. CreateCluster succeeds on nonexistent subnets and hands back a ClusterArn, so the caller is left holding a handle to a cluster whose brokers can never be placed.
Why: subnet resolution is the main input-validation gate on cluster creation - it is what catches a stale, mistyped or wrong-region subnet id in a template or a Terraform plan. An emulator that accepts it turns a deploy-time 400 (loud, with the offending input named) into a cluster that is created and then never becomes usable, and a caller that treats the returned ARN as proof of a valid network configuration proceeds to wire security groups, bootstrap brokers and consumers around a cluster that cannot exist. It is also an over-permissiveness that cannot be defended against client-side: there is no way for the caller to tell a sandbox 200 from an AWS 200.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/create_cluster.rs (and create_cluster_v2.rs, which takes the same BrokerNodeGroupInfo in its Provisioned block) must resolve every BrokerNodeGroupInfo.ClientSubnets entry against the EC2 subnet state for the request's account and region, and refuse with a client fault - writing no cluster record - when any of them does not resolve. The refusal must happen before the ARN is minted, so no partially-created cluster is enumerable.
Traces: kafka.create-with-invalid-input-should-fail.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

kafka.create-with-invalid-input-should-fail (mismatch). The check's positive control runs first - a legitimately named create_configuration succeeds - so the account and credentials are known good and the expected rejection is attributable to the input, not to a blanket denial.
  create_cluster(ClusterName='sim2real-behav-bad-cluster-<rand>', KafkaVersion=<an ACTIVE version from ListKafkaVersions>, NumberOfBrokerNodes=2,
                 BrokerNodeGroupInfo={'InstanceType': 'kafka.m5.large',
                                      'ClientSubnets': ['subnet-00000000000000000', 'subnet-11111111111111111'],
                                      'SecurityGroups': ['sg-00000000000000000'],
                                      'StorageInfo': {'EbsStorageInfo': {'VolumeSize': 1}}})
  expected: one of AccessDeniedException / BadRequestException / ForbiddenException / NotFoundException / UnauthorizedException / ValidationException / ConflictException, and no cluster named with that prefix in ListClustersV2
  actual   AssertionError: CreateCluster(dead subnets) SUCCEEDED on an invalid target -- overly permissive; real AWS rejects this

(The check goes on to probe CreateClusterV2 with neither the Provisioned nor the Serverless block, CreateConfiguration with an illegal name, and CreateTopic/CreateVpcConnection/CreateReplicator parented on a never-issued cluster ARN; it aborts at the first case, so only the CreateCluster leg is evidenced here.)

## Steps to Reproduce
1. Boot the emulator.
2. Positive control: kafka.create_configuration(Name='good-config', ServerProperties=b'auto.create.topics.enable=true\n') -> expect 200, confirming the account is not denied everything.
3. kafka.create_cluster(ClusterName='bad-cluster', KafkaVersion=<an ACTIVE version reported by kafka.list_kafka_versions()>, NumberOfBrokerNodes=2, BrokerNodeGroupInfo={'InstanceType':'kafka.m5.large','ClientSubnets':['subnet-00000000000000000','subnet-11111111111111111'],'SecurityGroups':['sg-00000000000000000'],'StorageInfo':{'EbsStorageInfo':{'VolumeSize':1}}}) -> expect a client-fault rejection and NO ClusterArn in the response. Observe HTTP 200 with a ClusterArn.
4. Paginate kafka.list_clusters_v2() -> no cluster named 'bad-cluster' may be present.
5. Repeat step 3 against create_cluster_v2 with the same subnets inside its Provisioned block -> expect the same rejection.
6. Re-replay kafka.create-with-invalid-input-should-fail.
