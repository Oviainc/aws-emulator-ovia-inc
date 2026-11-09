Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.replicator-duplicate-name-conflict kafka.replicator-same-source-target-rejected kafka.tag-resource-reserved-key-rejected kafka.tag-untag-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateReplicator|BadRequestException
Issue-Commit: 4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12

# CreateReplicator is not asynchronous: a well-formed-but-nonexistent subnet/cluster reference is refused synchronously, and the replicator refusal/conflict wording diverges (4 causes)

> _Note: this issue was identified against the code base at commit `4c956511a30d233b8b8a2d2bfa41b4b7e6ed0d12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicator

## Description
FOUR distinct root causes on MSK's CreateReplicator path. Cause 1 is the blocker: the handler resolves the submitted subnets against EC2 inventory BEFORE minting and refuses, so a request real MSK accepts never produces a replicator at all. Because a replicator is the only taggable MSK resource a check can stand up without a network, Cause 1 also takes down both tagging checks in this suite. Causes 2-4 are the contract that becomes observable once Cause 1 is fixed, and each must be closed separately or the same checks fail again at the next assertion.

Related open issues that do NOT cover these: 20261029T202809Z-kafka-msk-create-and-update-inputs-aws-refuses-are.md (its CreateReplicator cause is the subnetIds CARDINALITY gate answering 200, the opposite direction from Cause 1 here, and it names no behavioral check); 20261029T175822Z-kafka-msk-error-messages-are-the-emulator-s-own.md and 20261029T202809Z-kafka-msk-not-found-already-exists-and-tagging-refusals.md (neither carries a replicator create/conflict message key); 20261029T175822Z-kafka-updatereplicationinfo-resolves-the-replicator-arn-before-validating-its.md (UpdateReplicationInfo only).

Cause 1 of 4 - CreateReplicator refuses a well-formed subnet id the account does not hold, instead of accepting the request and validating references asynchronously.
AWS: Smithy kafka#CreateReplicator (POST /replication/v1/replicators, success 200) requires KafkaClusters, ReplicationInfoList, ReplicatorName, ServiceExecutionRoleArn, and models BadRequestException (httpError 400) / NotFoundException (404). The model places no existence requirement on KafkaClusterClientVpcConfig$SubnetIds or on MskClusterArn. Recorded behavior: MSK resolves the referenced clusters and their client subnets ASYNCHRONOUSLY - a create naming syntactically valid but never-issued subnet ids (subnet-<17 hex>) and never-issued MSK cluster ARNs returns HTTP 200 with ReplicatorArn (arn:aws:kafka:...) and ReplicatorState 'CREATING'. The request is accepted; the bad references surface later, on the replicator's own state (see Cause 2).
Emulator: HTTP 400 BadRequestException on the create, invalidParameter 'KafkaClusters[0].VpcConfig.SubnetIds', message of the form "The subnet ID '<id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <id>; Proxy: null)" - the subnet id is taken from the request, the parenthesised EC2 envelope and request id are the emulator's own. No replicator record is written. The handler states the intent at services/service-com-amazonaws-kafka-kafka/src/ops/create_replicator.rs lines 16-22 and implements it at lines 339-366 (crate::state::network::placement_subnet_refusal).
Why: every client workflow that creates a replicator before its VPC plumbing is resolvable - and every negative test that uses a reference-only replicator as a cheap taggable resource - gets a hard 400 where AWS returns a CREATING handle. The caller cannot reach the replicator state machine, DescribeReplicator, ListReplicators, TagResource/UntagResource/ListTagsForResource on a replicator, or DeleteReplicator at all.
Fix: remove the synchronous subnet-existence gate from the CreateReplicator handler (services/service-com-amazonaws-kafka-kafka/src/ops/create_replicator.rs) and move cluster/subnet reference resolution onto the replicator's asynchronous state transition. Keep the request-only gates (list ceilings, tags, cluster identification, flow shape) - they are synchronous in AWS too.
Traces: kafka.replicator-duplicate-name-conflict, kafka.replicator-same-source-target-rejected, kafka.tag-resource-reserved-key-rejected, kafka.tag-untag-nonexistent-resource-rejected.

Cause 2 of 4 - an accepted replicator whose clusters do not exist never settles FAILED with StateInfo.
AWS: Smithy kafka#CreateReplicator output ReplicatorState is the ReplicatorState enum (CREATING | DELETING | FAILED | RUNNING | UPDATING); DescribeReplicator reports StateInfo. Recorded behavior: a replicator created against never-issued cluster ARNs leaves CREATING and settles in FAILED, and DescribeReplicator's StateInfo is {Code: 'InvalidInput.InvalidKafkaCluster', Message: 'The specified Amazon MSK Cluster does not exist. Cluster ARN: <source cluster ARN>'}. Fixed text: the Code string, and 'The specified Amazon MSK Cluster does not exist. Cluster ARN: '; variable: the source cluster ARN, taken verbatim from the request. DeleteReplicator refuses a replicator that is still CREATING, so this settle is also what makes the replicator deletable.
Emulator: unobservable - the create never returns a replicator (Cause 1), and the handler explicitly declines to resolve cluster references at all (same file, lines 11-14) with no deferred validation that could ever set a FAILED state or a StateInfo. The record is minted directly via crate::state::Replicator::provisioned and returned.
Why: a client that polls out of CREATING to learn whether its replicator is viable - the documented way to find out, since the create is asynchronous - either waits forever or is never told WHY the replicator is unusable, and cannot delete it.
Fix: model the asynchronous cluster-reference resolution as a replicator state transition in services/service-com-amazonaws-kafka-kafka/src/state.rs (CREATING -> FAILED) that records the StateInfo code/message, and serve it from the DescribeReplicator handler.
Traces: kafka.replicator-duplicate-name-conflict, kafka.replicator-same-source-target-rejected, kafka.tag-resource-reserved-key-rejected, kafka.tag-untag-nonexistent-resource-rejected.

Cause 3 of 4 - the duplicate-ReplicatorName conflict carries the emulator's own sentence and blames a parameter.
AWS: CreateReplicator models ConflictException (httpError 409). Recorded behavior: a second CreateReplicator under a ReplicatorName already in use in the account and region is refused with Error.Code 'ConflictException' and Error.Message exactly 'A resource with this name already exists.' - all fixed text; the name and the existing replicator's ARN are NOT echoed. The refusal is idempotent: an immediate retry returns the identical code and message, and ListReplicators(ReplicatorNameFilter=<name>) still shows exactly one replicator.
Emulator: ConflictException with invalidParameter 'ReplicatorName' and a message of the form "A replicator with the name '<name>' already exists: <existing ARN>" (services/service-com-amazonaws-kafka-kafka/src/ops/create_replicator.rs lines 373-384) - the emulator's own prose, and it leaks the existing replicator's ARN, which AWS withholds.
Why: a client that matches on the message to distinguish a name collision from other conflicts never matches, and one that parses the message gets an ARN field AWS does not supply, so code written against the emulator breaks on AWS.
Fix: the conflict branch of the CreateReplicator handler - emit the fixed AWS sentence and drop invalidParameter and the ARN interpolation.
Traces: kafka.replicator-duplicate-name-conflict.

Cause 4 of 4 - the same-source-and-target refusal is evaluated per flow on a derived alias and worded by the emulator, where AWS refuses the duplicate ARN in the kafkaClusters LIST.
AWS: CreateReplicator models BadRequestException (400). Recorded behavior: a create whose KafkaClusters entries carry the same MskClusterArn twice (so a flow's SourceKafkaClusterArn equals its TargetKafkaClusterArn) is refused 400 with Error.Code 'BadRequestException' and Error.Message exactly 'Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique.' - all fixed text; no ARN, alias or index is interpolated. The rule is a structural duplicate check on the cluster LIST, enforced synchronously and independently of whether the cluster resolves at all, and no replicator may be left under the requested name (ListReplicators must not show it).
Emulator: BadRequestException with invalidParameter 'ReplicationInfoList[<index>]' and a message of the form "The source and target of a replication flow must be different Kafka clusters; both resolve to the alias '<alias>'." (same file, lines 303-314) - the emulator's own prose, keyed to a per-flow derived cluster ALIAS rather than to the duplicated ARN in the cluster list, and carrying an index AWS does not report.
Why: the refusal AWS gives names the input the caller must change (a duplicate ARN in kafkaClusters); the emulator's names an internal alias concept, so remediation is pointed at the wrong member, and a client matching the documented sentence never matches. Keying on the alias also means two DIFFERENT cluster ARNs that happen to derive the same alias would be refused where AWS accepts them.
Fix: replace the per-flow alias comparison in services/service-com-amazonaws-kafka-kafka/src/ops/create_replicator.rs with a duplicate-ARN check over the KafkaClusters list, emitting the fixed AWS sentence.
Traces: kafka.replicator-same-source-target-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 43/215 passed. Four checks fail on this path; two as verdict=error (the CreateReplicator call raised before any assertion), two as verdict=mismatch (the check's own guard converted the rejection into an AssertionError).

Cause 1 - kafka.tag-resource-reserved-key-rejected (error) and kafka.tag-untag-nonexistent-resource-rejected (error):
  create_replicator(ReplicatorName=<new>, ServiceExecutionRoleArn='arn:aws:iam::<acct>:role/<new>', KafkaClusters=[{AmazonMskCluster:{MskClusterArn:<ghost>}, VpcConfig:{SubnetIds:['subnet-0a1b2c3d4e5f60718','subnet-0a1b2c3d4e5f60719'], SecurityGroupIds:['sg-0a1b2c3d4e5f6071a']}} x2], ReplicationInfoList=[one flow], Tags={'sim2real:managed':'true'})
  expected HTTP 200, ReplicatorArn set, ReplicatorState 'CREATING'
  actual   botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the CreateReplicator operation: The subnet ID 'subnet-0a1b2c3d4e5f60718' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <uuid>; Proxy: null)
  Consequence: neither check ever reached its subject. kafka.tag-resource-reserved-key-rejected never exercised TagResource with the reserved 'aws:' key (expected BadRequestException / 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'); kafka.tag-untag-nonexistent-resource-rejected passed its ghost-ARN leg and then died on the positive control.

Cause 1 - kafka.replicator-duplicate-name-conflict (mismatch):
  AssertionError: environment does not accept an async CreateReplicator against ghost clusters: BadRequestException
Cause 1 - kafka.replicator-same-source-target-rejected (mismatch):
  AssertionError: positive control failed: CreateReplicator with distinct cluster refs was rejected (BadRequestException); a same-source-target rejection would not be separable from a globally broken CreateReplicator call

Cause 2 - asserted by both replicator checks and by the tagging checks' _create_settled_replicator helper:
  describe_replicator(ReplicatorArn=<arn>) polled out of CREATING must report ReplicatorState 'FAILED' and StateInfo == {'Code': 'InvalidInput.InvalidKafkaCluster', 'Message': 'The specified Amazon MSK Cluster does not exist. Cluster ARN: <source ARN from the request>'}. Unreachable today; no deferred validation exists in the handler (create_replicator.rs lines 11-14 record that cluster references are deliberately never resolved).

Cause 3 - kafka.replicator-duplicate-name-conflict asserts, for both the first duplicate and an immediate retry:
  expected Error.Code 'ConflictException', Error.Message 'A resource with this name already exists.'
  emulator source (create_replicator.rs:373-384) builds ConflictException{invalid_parameter: Some("ReplicatorName"), message: "A replicator with the name '<name>' already exists: <existing arn>"}

Cause 4 - kafka.replicator-same-source-target-rejected asserts:
  expected Error.Code 'BadRequestException', Error.Message 'Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique.', and the requested ReplicatorName absent from ListReplicators
  emulator source (create_replicator.rs:303-314) builds BadRequestException{invalid_parameter: Some("ReplicationInfoList[<i>]"), message: "The source and target of a replication flow must be different Kafka clusters; both resolve to the alias '<alias>'."}

Smithy cited: kafka#CreateReplicator POST /replication/v1/replicators, success 200; required KafkaClusters / ReplicationInfoList / ReplicatorName (len 1..128, pattern ^[0-9A-Za-z][0-9A-Za-z-]{0,}$) / ServiceExecutionRoleArn; output ReplicatorArn, ReplicatorName, ReplicatorState (CREATING|DELETING|FAILED|RUNNING|UPDATING); errors BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500), NotFoundException(404), ServiceUnavailableException(503), TooManyRequestsException(429), UnauthorizedException(401). No existence constraint is modeled on SubnetIds or MskClusterArn.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: kafka.create_replicator with two KafkaClusters entries naming distinct well-formed-but-never-issued MSK cluster ARNs, each with VpcConfig.SubnetIds=['subnet-0a1b2c3d4e5f60718','subnet-0a1b2c3d4e5f60719'] and SecurityGroupIds=['sg-0a1b2c3d4e5f6071a'], one matching ReplicationInfoList flow, and a never-issued ServiceExecutionRoleArn -> expect HTTP 200 with ReplicatorArn beginning 'arn:aws:kafka:' and ReplicatorState 'CREATING'. Observe 400 BadRequestException naming the subnet id.
3. Cause 2: poll describe_replicator on that ARN until ReplicatorState != 'CREATING' -> expect 'FAILED' with StateInfo {'Code':'InvalidInput.InvalidKafkaCluster','Message':'The specified Amazon MSK Cluster does not exist. Cluster ARN: <the SourceKafkaClusterArn sent>'}; then delete_replicator must succeed (and must refuse while still CREATING).
4. Cause 3: repeat the step-2 create with the SAME ReplicatorName -> expect 409 ConflictException, Message exactly 'A resource with this name already exists.' with no invalidParameter and no ARN in the text; repeat once more and assert the identical code and message; then list_replicators(ReplicatorNameFilter=<name>) must show exactly one replicator.
5. Cause 4: issue the step-2 create with the SAME cluster ARN as both KafkaClusters entries and as both SourceKafkaClusterArn and TargetKafkaClusterArn -> expect 400 BadRequestException, Message exactly 'Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique.', and the requested ReplicatorName absent from list_replicators.
6. Regression for the tagging checks once Cause 1 is fixed: tag_resource on the settled replicator with {'positive-control':'ok'} must succeed and round-trip; tag_resource with {'aws:reserved':'nope'} must answer BadRequestException / 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)' and must not store the key or disturb the existing tag.
7. Re-replay every check id in trace_ids.
