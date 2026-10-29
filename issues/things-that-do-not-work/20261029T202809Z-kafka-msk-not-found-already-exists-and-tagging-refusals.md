Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-describe-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-describe-not-found kafka-replicator-not-found-and-validation-errors kafka-replicator-notfound-errors kafka-replicator-notfound-paths kafka-configuration-error-paths kafka-replicator-create-describe-delete-lifecycle kafka-tagging-error-paths kafka-vpc-connection-create-nonexistent-target-cluster kafka-vpc-connection-crud-notfound-error-paths kafka-batch-associate-malformed-secret-arn kafka-create-replicator-same-source-and-target-cluster kafka-replicator-crud-notfound-error-paths kafka-replicator-validation-errors
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DescribeClusterV2|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DescribeClusterV2|4xx->4xx|$.message
Issue-Key: kafka|DescribeReplicator|4xx->4xx|$.message
Issue-Key: kafka|CreateConfiguration|4xx->4xx|$.message
Issue-Key: kafka|CreateReplicator|4xx->4xx|$.invalidParameter
Issue-Key: kafka|CreateReplicator|4xx->4xx|$.message
Issue-Key: kafka|TagResource|4xx->4xx|$.message
Issue-Key: kafka|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|TagResource|4xx->4xx|status
Issue-Key: kafka|UntagResource|4xx->4xx|$.message
Issue-Key: kafka|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|UntagResource|4xx->4xx|status
Issue-Key: kafka|CreateVpcConnection|4xx->4xx|$.invalidParameter
Issue-Key: kafka|CreateVpcConnection|4xx->4xx|$.message
Issue-Key: kafka|CreateVpcConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateVpcConnection|4xx->4xx|status
Issue-Key: kafka|BatchAssociateScramSecret|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|BatchAssociateScramSecret|4xx->4xx|status
Issue-Key: kafka|BatchDisassociateScramSecret|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|BatchDisassociateScramSecret|4xx->4xx|status
Issue-Related: 20261029T202809Z-kafka-msk-invalid-value-refusals-on-operation-secret-vpc.md
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK not-found, already-exists and tagging refusals use the wrong class and wording (10 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterV2

## Description
TEN distinct root causes on MSK's resource-resolution refusals. Four of them get the HTTP STATUS and error code wrong (a client dispatching on 400-vs-404 is routed to the wrong remedy), and six get the message text wrong where MSK uses fixed, resource-opaque sentences. All ten must be closed.

Cause 1 of 10 - DescribeClusterV2/DescribeCluster not-found message.
AWS: Smithy kafka#DescribeClusterV2 models NotFoundException (httpError 404). A well-formed, never-issued cluster ARN is refused 404 / NotFoundException with `invalidParameter` 'clusterArn' and `message` exactly "The requested resource doesn't exist." (apostrophe U+2019) - the entire sentence is fixed text; neither the ARN nor the region is echoed.
Emulator: 404 / NotFoundException, but invalidParameter 'ClusterArn' and a composed, request-derived sentence of the form "No cluster exists in <region> with the ARN <clusterArn>".
Why: a client comparing or logging MSK's refusal text sees a string AWS never emits, and the emulator leaks the request's region and ARN into a constant message.
Fix: the not-found refusal in services/service-com-amazonaws-kafka-kafka/src/ops/describe_cluster_v2.rs (and describe_cluster.rs) - better, the one shared NotFoundException constructor.
Traces: kafka-cluster-describe-not-found#task#1.

Cause 2 of 10 - DescribeReplicator not-found message is prefixed.
AWS: DescribeReplicator models NotFoundException (404); the message is exactly the same fixed sentence "The requested resource doesn't exist." with NO parameter prefix.
Emulator: 404 / NotFoundException with "Invalid parameter replicatorArn: The requested resource doesn't exist." - the fixed sentence is correct but prefixed with the emulator's own "Invalid parameter <name>: " framing, which AWS puts in `invalidParameter`, not in `message`.
Why: the message no longer equals MSK's string, so exact-match classification fails; the prefix also contradicts the payload's own invalidParameter member.
Fix: stop prefixing `message` in the replicator ARN resolution path (ops/describe_replicator.rs and the shared replicator-ARN resolver) - put the parameter name in `invalidParameter`.
Traces: kafka-replicator-not-found-and-validation-errors#task#1.

Cause 3 of 10 - CreateConfiguration duplicate-name message.
AWS: Smithy kafka#CreateConfiguration models ConflictException (409). A Name already taken is refused with `message` exactly "A resource with this name already exists." - all fixed text; the offending name and the existing ARN are NOT disclosed.
Emulator: 409 / ConflictException with "A configuration with the name '<name>' already exists: <existing arn>".
Why: create-or-reuse clients that recognise the duplicate by MSK's message fail; the emulator also discloses an existing resource's ARN in an error AWS keeps opaque.
Fix: the name-uniqueness path in ops/create_configuration.rs.
Traces: kafka-configuration-error-paths#task#2.

Cause 4 of 10 - CreateReplicator duplicate-name message and parameter.
AWS: Smithy kafka#CreateReplicator models ConflictException (409); the message is the SAME service-wide fixed sentence as Cause 3, and `invalidParameter` is 'replicatorName'.
Emulator: invalidParameter 'ReplicatorName' and "A replicator with the name '<name>' already exists: <existing arn>".
Why: same as Cause 3, and it shows the duplicate-name text is written per resource type instead of once for the service.
Fix: ops/create_replicator.rs; share the conflict constructor with create_configuration.rs.
Traces: kafka-replicator-create-describe-delete-lifecycle#task#3.

Cause 5 of 10 - TagResource on an unusable ResourceArn answers 404 instead of 400.
AWS: Smithy kafka#TagResource (POST /v1/tags/{ResourceArn}, success 204) models BadRequestException (400) and NotFoundException (404). Recorded behavior: a ResourceArn the service cannot accept as a taggable MSK ARN is an INPUT error - HTTP 400 with `message` exactly "Invalid input resource arn" (all fixed text, no value echoed), not a resource lookup miss.
Emulator: HTTP 404 with x-amzn-errortype NotFoundException and "Resource was not found".
Why: the status class is wrong - a tagging client retries or escalates on 404 (resource may appear later) but must fix its own request on 400; and both the code and the sentence are the emulator's own.
Fix: validate the ResourceArn before resolution in ops/tag_resource.rs and emit BadRequestException with the fixed string.
Traces: kafka-tagging-error-paths#task#2.

Cause 6 of 10 - UntagResource on an unusable ResourceArn answers 404 instead of 400.
AWS: Smithy kafka#UntagResource (DELETE /v1/tags/{ResourceArn}, success 204) models the same two errors; the recorded refusal is 400 with "Invalid input resource arn".
Emulator: 404 NotFoundException "Resource was not found".
Why: as Cause 5; listed separately because it is a second handler composing its own refusal.
Fix: ops/untag_resource.rs, sharing the validator with tag_resource.rs.
Traces: kafka-tagging-error-paths#task#3.

Cause 7 of 10 - CreateVpcConnection against a nonexistent target cluster answers 400 instead of 404.
AWS: recorded behavior only for the error class - Smithy kafka#CreateVpcConnection (POST /v1/vpc-connection) models BadRequestException/Forbidden/Unauthorized but not NotFoundException, yet the recording shows a well-formed TargetClusterArn naming no cluster is refused HTTP 404 with x-amzn-errortype NotFoundException, `invalidParameter` 'clusterArn' and `message` "The requested resource doesn't exist." (fixed text).
Emulator: HTTP 400 BadRequestException, invalidParameter 'TargetClusterArn', message "No cluster exists in <region> with the ARN <arn>".
Why: the status class is wrong, so a client cannot tell 'the cluster is not there yet' (retryable/eventually-consistent) from 'my request is malformed'.
Fix: ops/create_vpc_connection.rs must treat an unresolvable target cluster as a not-found, blaming 'clusterArn'.
Traces: kafka-vpc-connection-create-nonexistent-target-cluster#task#1.

Cause 8 of 10 - BatchAssociateScramSecret runs secret-ARN shape validation before the cluster lookup.
AWS: Smithy kafka#BatchAssociateScramSecret models BadRequestException (400) AND NotFoundException (404). Recorded behavior: when the ClusterArn names no cluster, the cluster miss is reported FIRST - HTTP 404 NotFoundException, `invalidParameter` 'clusterArn', `message` "The requested resource doesn't exist." - even though the SecretArnList entry is also unusable.
Emulator: HTTP 400 BadRequestException, invalidParameter 'SecretArnList', message "<secret value> is not a valid AWS Secrets Manager secret ARN." - it validates the secret list before resolving the cluster, so the gate ORDER is inverted and the caller is told to fix the wrong input.
Why: a client cleaning up SCRAM associations against a deleted cluster is told its secret ARN is malformed; it will 'fix' a correct input and never discover the cluster is gone.
Fix: resolve ClusterArn before validating SecretArnList in ops/batch_associate_scram_secret.rs.
Traces: kafka-batch-associate-malformed-secret-arn#task#1.

Cause 9 of 10 - BatchDisassociateScramSecret has the same inverted gate order.
AWS: as Cause 8, for kafka#BatchDisassociateScramSecret (PATCH /v1/clusters/{ClusterArn}/scram-secrets): 404 NotFoundException, invalidParameter 'clusterArn', fixed message.
Emulator: 400 BadRequestException blaming 'SecretArnList'.
Fix: ops/batch_disassociate_scram_secret.rs; order the gates the same way as Cause 8.
Traces: kafka-batch-associate-malformed-secret-arn#task#2.

Cause 10 of 10 - CreateReplicator duplicate-cluster refusal states the wrong rule.
AWS: CreateReplicator models BadRequestException (400). When the kafkaClusters list names the same cluster twice, the refusal is `invalidParameter` 'kafkaClusters' and `message` exactly "Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique." - all fixed text; the duplicated ARN is not named. The rule is about the cluster ARN list being a set.
Emulator: invalidParameter 'ReplicationInfoList[0]' and "The source and target of a replication flow must be different Kafka clusters; both resolve to the alias '<name>'." - a different parameter, a different rule and different wording.
Why: the client is pointed at the wrong request member, so a generated fix edits replicationInfoList when kafkaClusters is what must change.
Fix: validate kafkaClusters for ARN uniqueness in ops/create_replicator.rs and emit the fixed sentence.
Traces: kafka-create-replicator-same-source-and-target-cluster#task#1.

Neighbouring filings: 20261029T175822Z-kafka-msk-error-messages-are-the-emulator-s-own.md documents Causes 1, 3, 4 and 10 as BEHAVIORAL check failures and 20261029T175822Z-kafka-msk-arn-handling-malformed-and-cross-region-arns.md documents the gate-order family; neither carries any of the wire fixture#step keys above, so re-replaying them is not gated by either.

## Evidence
Corpus: simparity-wire-aws-v1. All ten clusters verdict=mismatch.

Cause 1 - kafka-cluster-describe-not-found#task#1, DescribeClusterV2, 404 == 404:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'
  $.message: expected 'The requested resource doesn\u2019t exist.', got 'No cluster exists in us-west-2 with the ARN arn:aws:kafka:us-west-2:000000000000:cluster/<name>/00000000-0000-0000-0000-000000000000-1'

Cause 2 - kafka-replicator-not-found-and-validation-errors#task#1, DescribeReplicator, 404 == 404:
  $.message: expected 'The requested resource doesn\u2019t exist.', got 'Invalid parameter replicatorArn: The requested resource doesn\u2019t exist.'
  (cluster also affects kafka-replicator-notfound-errors#task#1, kafka-replicator-notfound-paths#task#1)

Cause 3 - kafka-configuration-error-paths#task#2, CreateConfiguration, 409 == 409:
  $.message: expected 'A resource with this name already exists.', got "A configuration with the name 'sim2real-probe-kafka-config-conflict-<suffix>' already exists: arn:aws:kafka:...:configuration/<name>/<uuid>-1"

Cause 4 - kafka-replicator-create-describe-delete-lifecycle#task#3, CreateReplicator, 409 == 409:
  $.invalidParameter: expected 'replicatorName', got 'ReplicatorName'
  $.message: expected 'A resource with this name already exists.', got "A replicator with the name '<name>' already exists: arn:aws:kafka:...:replicator/<name>/<uuid>-1"

Cause 5 - kafka-tagging-error-paths#task#2, TagResource: status expected 400, got 404
  $.message: expected 'Invalid input resource arn', got 'Resource was not found'
  $headers.x-amzn-errortype: expected a shaped BadRequestException, got 'NotFoundException'

Cause 6 - kafka-tagging-error-paths#task#3, UntagResource: identical to Cause 5.

Cause 7 - kafka-vpc-connection-create-nonexistent-target-cluster#task#1, CreateVpcConnection: status expected 404, got 400
  $.invalidParameter: expected 'clusterArn', got 'TargetClusterArn'
  $.message: expected 'The requested resource doesn\u2019t exist.', got 'No cluster exists in us-west-2 with the ARN arn:aws:kafka:us-west-2:000000000000:cluster/<ghost>/00000000-0000-0000-0000-000000000000-1'
  $headers.x-amzn-errortype: expected 'NotFoundException', got 'BadRequestException'
  (cluster also affects kafka-vpc-connection-crud-notfound-error-paths#task#2)

Cause 8 - kafka-batch-associate-malformed-secret-arn#task#1, BatchAssociateScramSecret: status expected 404, got 400
  $.invalidParameter: expected 'clusterArn', got 'SecretArnList'
  $.message: expected 'The requested resource doesn\u2019t exist.', got 'not-a-secret-arn is not a valid AWS Secrets Manager secret ARN.'
  $headers.x-amzn-errortype: expected 'NotFoundException', got 'BadRequestException'

Cause 9 - kafka-batch-associate-malformed-secret-arn#task#2, BatchDisassociateScramSecret: identical to Cause 8.

Cause 10 - kafka-create-replicator-same-source-and-target-cluster#task#1, CreateReplicator, 400 == 400:
  $.invalidParameter: expected 'kafkaClusters', got 'ReplicationInfoList[0]'
  $.message: expected 'Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique.', got "The source and target of a replication flow must be different Kafka clusters; both resolve to the alias '<cluster name>'."
  (cluster also affects kafka-replicator-crud-notfound-error-paths#task#1, kafka-replicator-validation-errors#task#1)

Smithy cited: kafka#TagResource POST /v1/tags/{ResourceArn} code 204, errors BadRequestException(400)/NotFoundException(404); kafka#UntagResource DELETE same path, same errors; kafka#CreateVpcConnection POST /v1/vpc-connection, errors BadRequestException(400)/Forbidden(403)/Unauthorized(401) - no NotFoundException modeled, so Cause 7's 404 is recorded behavior only; kafka#BatchAssociateScramSecret and kafka#BatchDisassociateScramSecret both model BadRequestException(400) and NotFoundException(404); kafka#CreateReplicator input member KafkaClusters jsonName 'kafkaClusters', ReplicatorName jsonName 'replicatorName'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: replay kafka-cluster-describe-not-found#task#1 - DescribeClusterV2 on a never-issued cluster ARN must answer 404 with invalidParameter 'clusterArn' and message exactly 'The requested resource doesn\u2019t exist.' (U+2019), with no region or ARN in the text.
3. Cause 2: kafka-replicator-not-found-and-validation-errors#task#1 - DescribeReplicator on a never-issued replicator ARN must answer that same unprefixed sentence.
4. Causes 3/4: kafka-configuration-error-paths#task#2 and kafka-replicator-create-describe-delete-lifecycle#task#3 - a duplicate Name/ReplicatorName must answer 409 with message exactly 'A resource with this name already exists.' and no existing ARN disclosed.
5. Causes 5/6: kafka-tagging-error-paths#task#2 and #task#3 - Tag/UntagResource with an unusable ResourceArn must answer HTTP 400 BadRequestException with message 'Invalid input resource arn'.
6. Cause 7: kafka-vpc-connection-create-nonexistent-target-cluster#task#1 - CreateVpcConnection with a well-formed TargetClusterArn naming no cluster must answer HTTP 404 NotFoundException, invalidParameter 'clusterArn', fixed message.
7. Causes 8/9: kafka-batch-associate-malformed-secret-arn#task#1 and #task#2 - Batch(Dis)AssociateScramSecret with a nonexistent cluster AND an unusable secret entry must report the CLUSTER miss first: 404 NotFoundException, invalidParameter 'clusterArn'.
8. Cause 10: kafka-create-replicator-same-source-and-target-cluster#task#1 - CreateReplicator naming one cluster twice must answer 400 with invalidParameter 'kafkaClusters' and the fixed duplicate-ARN sentence.
9. Re-replay all ten fixture#step bindings.
