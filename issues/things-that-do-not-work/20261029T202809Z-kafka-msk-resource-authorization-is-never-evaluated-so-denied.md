Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-operations-and-policy-notfound-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-operations-and-policy-notfound-error-paths kafka-channel-error-paths-ghost-cluster kafka-cluster-mutations-notfound-2 kafka-topic-notfound-paths-ghost-cluster kafka-create-replicator-malformed-role-arn kafka-replicator-not-found-and-validation-errors
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DescribeClusterV2|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|DescribeClusterV2|4xx->4xx|status
Issue-Key: kafka|CreateChannel|4xx->4xx|$.message
Issue-Key: kafka|CreateChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateChannel|4xx->4xx|status
Issue-Key: kafka|DescribeChannel|4xx->4xx|$.message
Issue-Key: kafka|DescribeChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|DescribeChannel|4xx->4xx|status
Issue-Key: kafka|DeleteChannel|4xx->4xx|$.message
Issue-Key: kafka|DeleteChannel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|DeleteChannel|4xx->4xx|status
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$.message
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|status
Issue-Key: kafka|CreateTopic|4xx->4xx|$.message
Issue-Key: kafka|CreateTopic|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateTopic|4xx->4xx|status
Issue-Key: kafka|DescribeTopic|4xx->4xx|$.message
Issue-Key: kafka|DescribeTopic|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|DescribeTopic|4xx->4xx|status
Issue-Key: kafka|DeleteTopic|4xx->4xx|$.message
Issue-Key: kafka|DeleteTopic|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|DeleteTopic|4xx->4xx|status
Issue-Key: kafka|ListTopics|4xx->4xx|status
Issue-Key: kafka|CreateReplicator|4xx->2xx|$.message
Issue-Key: kafka|CreateReplicator|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateReplicator|4xx->2xx|status
Issue-Related: 20261029T202809Z-kafka-msk-not-found-already-exists-and-tagging-refusals.md
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK resource authorization is never evaluated, so denied requests answer not-found or succeed (10 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterV2

## Description
TEN distinct root causes sharing one fault: MSK evaluates resource-scoped authorization BEFORE it resolves the resource, so a request for a cluster the caller has no policy for is refused HTTP 403 AccessDeniedException. The emulator never runs that gate: it goes straight to the resource lookup (or straight to body validation) and answers 404/400 instead, and in one case answers 200 and mints a resource AWS refuses outright. A client cannot distinguish 'you may not touch this' (escalate to IAM) from 'it is not there' (retry/create), and the one 200 is a silent privilege escalation. All ten must be closed.

AWS (common to all ten): each operation below is cluster-scoped (ClusterArn is an httpLabel, or the service-execution role must be assumable). Recorded behavior: when the target cluster ARN names an account the caller holds no resource-based policy for, the refusal is HTTP 403 with header x-amzn-errortype 'AccessDeniedException' and `message` of the form "User: <caller principal ARN> is not authorized to perform: <service:Action> on resource: <the target resource ARN> because no resource-based policy allows the <service:Action> action". Fixed text: "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the ", " action". Variable parts, all taken from the request and the evaluated policy: the caller's principal ARN, the IAM action name for the operation (kafka:<Operation>, or kafka-cluster:Connect for the topic data-plane operations), and the target resource ARN. The recorded run's principal and ARN values are one filling of that template, not part of it. Note the Smithy model lists ForbiddenException (403) and UnauthorizedException (401) rather than an AccessDeniedException shape, so the error code itself is recorded behavior only; the 403 status is modeled.

Emulator (common to all ten): no authorization evaluation happens for the path/target ARN, so the handler's own resolution answers first: 404 NotFoundException "The requested resource doesn't exist." for the lookup-miss operations, 400 BadRequestException with a body-validation complaint for CreateChannel, and HTTP 200 for CreateReplicator with a cross-account service-execution role.

Why (common to all ten): least-privilege and cross-account behavior cannot be exercised against the emulator at all - every denial reads as absence. Worse, Cause 10 lets a request AWS refuses create real state, so a test suite that asserts cross-account pass-role is blocked passes against AWS and silently fails open here.

Fix (common to all ten): the request-authorization layer must evaluate the operation's IAM action against the RAW target ARN before the handler resolves it, for every cluster-scoped operation in services/service-com-amazonaws-kafka-kafka/src/ - a per-handler fix leaves the rest diverging. The named handlers below are where resolution currently answers first.

Cause 1 of 10: DescribeClusterV2 on a cluster ARN in a foreign account. AWS 403 AccessDeniedException (action kafka:DescribeClusterV2); emulator 404 NotFoundException with its own "No cluster exists in <region> with the ARN <arn>". Fix: authorization before ops/describe_cluster_v2.rs resolution.
Traces: kafka-cluster-operations-and-policy-notfound-error-paths#task#2.

Cause 2 of 10: CreateChannel on a foreign-account cluster. AWS 403 AccessDeniedException (kafka:CreateChannel), evaluated BEFORE the request body is validated; emulator 400 BadRequestException "Exactly one of s3DestinationConfiguration and icebergDestinationConfiguration must be specified, not 0." - i.e. body validation runs where AWS has already denied. Fix: ops/create_channel.rs.
Traces: kafka-channel-error-paths-ghost-cluster#task#2.

Cause 3 of 10: DescribeChannel on a foreign-account cluster. AWS 403 AccessDeniedException (kafka:DescribeChannel); emulator 404 NotFoundException. Fix: ops/describe_channel.rs.
Traces: kafka-channel-error-paths-ghost-cluster#task#3.

Cause 4 of 10: DeleteChannel on a foreign-account cluster. AWS 403 AccessDeniedException (kafka:DeleteChannel); emulator 404 NotFoundException. Fix: ops/delete_channel.rs.
Traces: kafka-channel-error-paths-ghost-cluster#task#5.

Cause 5 of 10: UpdateConnectivity on a cluster the caller may not mutate. AWS 403 AccessDeniedException (kafka:UpdateConnectivity); emulator 404 NotFoundException. Fix: ops/update_connectivity.rs.
Traces: kafka-cluster-mutations-notfound-2#task#2.

Cause 6 of 10: CreateTopic on a foreign-account cluster. AWS 403 AccessDeniedException for the DATA-plane action kafka-cluster:Connect (not kafka:CreateTopic) - the topic operations authorize cluster connectivity; emulator 404 NotFoundException. Fix: ops/create_topic.rs.
Traces: kafka-topic-notfound-paths-ghost-cluster#task#2.

Cause 7 of 10: DescribeTopic, same gate and same kafka-cluster:Connect action. AWS 403; emulator 404 NotFoundException. Fix: ops/describe_topic.rs.
Traces: kafka-topic-notfound-paths-ghost-cluster#task#3.

Cause 8 of 10: DeleteTopic, same gate. AWS 403; emulator 404 NotFoundException. Fix: ops/delete_topic.rs.
Traces: kafka-topic-notfound-paths-ghost-cluster#task#6.

Cause 9 of 10: ListTopics, same gate. AWS 403; emulator 404. Note Smithy kafka#ListTopics models BadRequestException(400), ForbiddenException(403), Unauthorized(401) and the server errors - NotFoundException is NOT a modeled error for this operation at all, so the emulator's 404 is unmodeled. Fix: ops/list_topics.rs.
Traces: kafka-topic-notfound-paths-ghost-cluster#task#7.

Cause 10 of 10: CreateReplicator with a service-execution role in another account is ACCEPTED.
AWS: Smithy kafka#CreateReplicator requires ServiceExecutionRoleArn and models BadRequestException(400), ConflictException(409), ForbiddenException(403), NotFoundException(404). Recorded behavior: a ServiceExecutionRoleArn whose account differs from the caller's is refused HTTP 403, x-amzn-errortype 'AccessDeniedException', `message` exactly "Cross-account pass role is not allowed." - all fixed text, nothing request-derived.
Emulator: HTTP 200 - it mints the replicator, emitting no error body at all.
Why: this is the fail-open case. A client (or a security test) that relies on MSK refusing a foreign pass-role gets a working replicator, and the emulator's state now holds a resource AWS would never have created.
Fix: validate the account segment of ServiceExecutionRoleArn against the caller's account in ops/create_replicator.rs, before the replicator is stored.
Traces: kafka-create-replicator-malformed-role-arn#task#1.

Neighbouring filing: the open behavioral issue 20261029T175822Z-kafka-msk-arn-handling-malformed-and-cross-region-arns.md describes the same gate-ordering family for MALFORMED ARNs on DescribeCluster and the channel surface. It carries only behavioral check ids and none of the wire fixture#step keys above, and its causes are about ARN syntax rather than the foreign-account denial asserted here.

## Evidence
Corpus: simparity-wire-aws-v1. All ten clusters verdict=mismatch.

Cause 1 - kafka-cluster-operations-and-policy-notfound-error-paths#task#2, DescribeClusterV2: status expected 403, got 404
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'NotFoundException'
  $.message: expected 'User: arn:aws:iam::<caller-acct>:user/<caller> is not authorized to perform: kafka:DescribeClusterV2 on resource: <target cluster arn> because no resource-based policy allows the kafka:DescribeClusterV2 action', got 'No cluster exists in us-west-2 with the ARN <target cluster arn>'

Cause 2 - kafka-channel-error-paths-ghost-cluster#task#2, CreateChannel: status expected 403, got 400
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'BadRequestException'
  $.message: expected the kafka:CreateChannel denial sentence, got 'Exactly one of s3DestinationConfiguration and icebergDestinationConfiguration must be specified, not 0.'

Cause 3 - kafka-channel-error-paths-ghost-cluster#task#3, DescribeChannel: status expected 403, got 404; errortype expected 'AccessDeniedException', got 'NotFoundException'; message expected the kafka:DescribeChannel denial sentence, got 'The requested resource doesn\u2019t exist.'

Cause 4 - kafka-channel-error-paths-ghost-cluster#task#5, DeleteChannel: as Cause 3, action kafka:DeleteChannel.

Cause 5 - kafka-cluster-mutations-notfound-2#task#2, UpdateConnectivity: status expected 403, got 404; errortype expected 'AccessDeniedException', got 'NotFoundException'; message expected the kafka:UpdateConnectivity denial sentence, got 'The requested resource doesn\u2019t exist.'

Cause 6 - kafka-topic-notfound-paths-ghost-cluster#task#2, CreateTopic: status expected 403, got 404; errortype expected 'AccessDeniedException', got 'NotFoundException'; message expected the kafka-cluster:Connect denial sentence, got 'The requested resource doesn\u2019t exist.'

Cause 7 - kafka-topic-notfound-paths-ghost-cluster#task#3, DescribeTopic: identical to Cause 6.

Cause 8 - kafka-topic-notfound-paths-ghost-cluster#task#6, DeleteTopic: identical to Cause 6.

Cause 9 - kafka-topic-notfound-paths-ghost-cluster#task#7, ListTopics: status expected 403, got 404 (status is the only asserted key for this cluster).

Cause 10 - kafka-create-replicator-malformed-role-arn#task#1, CreateReplicator: status expected 403, got 200
  $.message: expected 'Cross-account pass role is not allowed.', got None (no error body)
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got None
  (cluster also affects kafka-replicator-not-found-and-validation-errors#task#2)

Smithy cited: kafka#ListTopics errors = BadRequestException(400), ForbiddenException(403), InternalServerError(500), ServiceUnavailable(503), UnauthorizedException(401) - no NotFoundException; kafka#DescribeChannel GET /v1/clusters/{ClusterArn}/channels/{ChannelArn} errors include ForbiddenException(403) and NotFoundException(404); kafka#CreateReplicator input ServiceExecutionRoleArn required, errors include ForbiddenException(403).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: replay kafka-cluster-operations-and-policy-notfound-error-paths#task#2 - DescribeClusterV2 on a cluster ARN whose account segment is not the caller's must answer HTTP 403 with x-amzn-errortype 'AccessDeniedException' and the "User: ... is not authorized to perform: kafka:DescribeClusterV2 on resource: ... because no resource-based policy allows the kafka:DescribeClusterV2 action" message, with the principal and resource ARN taken from the request/credentials.
3. Cause 2: kafka-channel-error-paths-ghost-cluster#task#2 - CreateChannel on that same foreign cluster, with a body that is ALSO invalid, must still answer the 403 denial (authorization before body validation), not the destination-configuration 400.
4. Causes 3/4: kafka-channel-error-paths-ghost-cluster#task#3 and #task#5 - DescribeChannel/DeleteChannel must answer 403 AccessDeniedException for kafka:DescribeChannel / kafka:DeleteChannel.
5. Cause 5: kafka-cluster-mutations-notfound-2#task#2 - UpdateConnectivity must answer 403 for kafka:UpdateConnectivity.
6. Causes 6-9: kafka-topic-notfound-paths-ghost-cluster steps task#2, task#3, task#6, task#7 - CreateTopic/DescribeTopic/DeleteTopic/ListTopics on the foreign cluster must each answer 403 naming the kafka-cluster:Connect action. ListTopics must never answer 404, which it does not model.
7. Cause 10: kafka-create-replicator-malformed-role-arn#task#1 - CreateReplicator with a ServiceExecutionRoleArn in another account must answer HTTP 403 AccessDeniedException with message exactly 'Cross-account pass role is not allowed.', and must NOT store a replicator (a following ListReplicators must not show it).
8. Re-replay all ten fixture#step bindings.
