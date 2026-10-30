Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-mutations-notfound-2
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-mutations-notfound-2 kafka-replicator-validation-errors
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$.message
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|status
Issue-Key: kafka|CreateReplicator|4xx->2xx|$.message
Issue-Key: kafka|CreateReplicator|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateReplicator|4xx->2xx|status
Issue-Related: 20261029T202809Z-kafka-msk-create-and-update-inputs-aws-refuses-are.md
Issue-Commit: 962bc15d984e86ce56d75221c3331a6151fbca59

# MSK refusal gates are skipped on mutation paths: UpdateConnectivity answers not-found instead of the authorization denial, and CreateReplicator accepts an out-of-range subnetIds list (2 causes)

> _Note: this issue was identified against the code base at commit `962bc15d984e86ce56d75221c3331a6151fbca59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnectivity

## Description
TWO distinct net-new root causes sharing one fault: on an MSK mutation path the emulator never reaches the refusal gate MSK answers from. In Cause 1 the cluster resource lookup answers before the authorization decision, so a denial is reported as an absence; in Cause 2 no input gate exists at all, so a request MSK refuses is accepted and state is minted. Both must be closed.

Cause 1 of 2: UpdateConnectivity on a cluster the caller is not authorized for answers 404 NotFoundException instead of the 403 authorization denial.
AWS: Smithy kafka#UpdateConnectivity, PUT /v1/clusters/{ClusterArn}/connectivity (success 200); ClusterArn is an @httpLabel and CurrentVersion is required; modeled errors include ForbiddenException (httpError 403), NotFoundException (httpError 404) and BadRequestException (400), each with members invalidParameter/message (jsonName spellings). Recorded behavior: when the ClusterArn names a cluster the caller holds no resource-based policy for, MSK refuses HTTP 403 with header x-amzn-errortype 'AccessDeniedException' and a message of the form "User: <caller principal ARN> is not authorized to perform: kafka:UpdateConnectivity on resource: <target cluster ARN> because no resource-based policy allows the kafka:UpdateConnectivity action". Fixed text, which must be reproduced verbatim: "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the ", " action". Variable parts, taken from the signing credentials and the request: the caller's principal ARN, the IAM action name (kafka:UpdateConnectivity, i.e. kafka: plus the operation name) and the cluster ARN from the ClusterArn httpLabel. The 403 status is modeled; the 'AccessDeniedException' error-type token itself is recorded behavior only (the model names ForbiddenException for 403). AWS answers the denial WITHOUT disclosing whether the cluster exists.
Emulator: HTTP 404, x-amzn-errortype 'NotFoundException', message 'The requested resource doesn’t exist.' — the handler resolves the cluster from state first and reports the miss, so no authorization verdict is ever produced for this operation.
Why: a client cannot distinguish 'you may not touch this cluster' (escalate to IAM / assume another role) from 'it is not there' (create it, or stop retrying). Cross-account and least-privilege workflows and their negative tests therefore pass against the emulator and fail on AWS, and the emulator leaks existence information AWS deliberately withholds.
Fix: authorize the operation's IAM action against the RAW ClusterArn label in the kafka request-authorization layer, before the UpdateConnectivity handler resolves the cluster from state — the same gate-ordering fix, applied to this operation. Related open issue that does NOT cover it: 20261030T011042Z-kafka-msk-create-and-update-paths-answer-the-wrong.md records this ordering fault for UpdateClusterConfiguration only and carries neither UpdateConnectivity nor this fixture#step. The issue that previously described the UpdateConnectivity authorization family, 20261029T202809Z-kafka-msk-resource-authorization-is-never-evaluated-so-denied.md, is RESOLVED, so it covers nothing; this replay step still diverges.
Traces: kafka-cluster-mutations-notfound-2#task#2.

Cause 2 of 2: CreateReplicator accepts a kafkaClusters[].vpcConfig.subnetIds list outside MSK's allowed count, answering HTTP 200 and minting a replicator.
AWS: Smithy kafka#CreateReplicator, POST /replication/v1/replicators (success 200), requires KafkaClusters, ReplicationInfoList, ReplicatorName and ServiceExecutionRoleArn; modeled errors include BadRequestException (httpError 400) with members invalidParameter/message. Recorded behavior: a create whose per-Kafka-cluster vpcConfig.subnetIds list carries too few or too many entries is refused HTTP 400 with x-amzn-errortype 'BadRequestException', invalidParameter 'subnetIds' (the modeled wire/jsonName spelling of the member, not a dotted Smithy path) and message exactly 'Too many or too few subnet ids specified.' — entirely fixed text: neither the submitted count, the offending subnet ids, nor the allowed bounds are interpolated. No replicator is created.
Emulator: HTTP 200 with a replicator minted — no error body at all, so invalidParameter, message and x-amzn-errortype are all absent.
Why: fail-open. A client's negative test for the subnet-count rule passes, and the emulator is then left holding a replicator over a VPC configuration MSK would never have admitted, so every later DescribeReplicator/ListReplicators read of it is fiction. Code written against the emulator ships and is refused 400 on AWS.
Fix: add the per-Kafka-cluster subnetIds cardinality gate to CreateReplicator's request validation, before any replicator state is written. Related open issue that does NOT cover this binding: 20261029T202809Z-kafka-msk-create-and-update-inputs-aws-refuses-are.md states the same rule as its Cause 3, but its replay key is kafka-replicator-notfound-errors#task#4; it carries no step binding for kafka-replicator-validation-errors, so this step is not gated by it and is reported net-new by the replay.
Traces: kafka-replicator-validation-errors#task#3.

## Evidence
Corpus: simparity-wire-aws-v1. 148 steps replayed, 69 passed, score 46.6%, 45 divergence clusters — all kafka. 43 of the 45 are already owned by open issues or blocked by a filed stub; these TWO clusters are the only ones with no stub_path and no owning open issue (owned_by: []). Both verdict=mismatch.

Cause 1 — kafka-cluster-mutations-notfound-2#task#2, UpdateConnectivity, status expected 403, got 404:
  status: expected 403, got 404
  $.message: expected 'User: <caller principal ARN> is not authorized to perform: kafka:UpdateConnectivity on resource: <cluster ARN from the ClusterArn label> because no resource-based policy allows the kafka:UpdateConnectivity action', got 'The requested resource doesn’t exist.'
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'NotFoundException'
  (In the recording the principal and cluster ARN are that run's values — an IAM user ARN and a cluster ARN with a generated uuid suffix. Those are request/credential-derived fill-ins, not requirements; the requirement is the 403, the 'AccessDeniedException' error type, and the fixed sentence skeleton with the caller principal, the kafka:UpdateConnectivity action and the submitted ClusterArn substituted in.)

Cause 2 — kafka-replicator-validation-errors#task#3, CreateReplicator, status expected 400, got 200:
  status: expected 400, got 200
  $.invalidParameter: expected 'subnetIds', got None
  $.message: expected 'Too many or too few subnet ids specified.', got None
  $headers.x-amzn-errortype: expected 'BadRequestException', got None

Smithy cited: kafka#UpdateConnectivity PUT /v1/clusters/{ClusterArn}/connectivity, ClusterArn @httpLabel required, CurrentVersion required, errors BadRequestException(400)/ForbiddenException(403)/NotFoundException(404)/UnauthorizedException(401)/InternalServerErrorException(500)/ServiceUnavailableException(503); kafka#CreateReplicator POST /replication/v1/replicators success 200, required KafkaClusters/ReplicationInfoList/ReplicatorName(len 1..128, pattern ^[0-9A-Za-z][0-9A-Za-z-]{0,}$)/ServiceExecutionRoleArn, errors BadRequestException(400)/ConflictException(409)/ForbiddenException(403)/NotFoundException(404)/TooManyRequestsException(429); kafka#BadRequestException, kafka#ForbiddenException and kafka#NotFoundException each carry InvalidParameter (jsonName 'invalidParameter') and Message (jsonName 'message').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: replay kafka-cluster-mutations-notfound-2#task#2 — UpdateConnectivity (PUT /v1/clusters/{ClusterArn}/connectivity, with currentVersion and connectivityInfo) against a cluster ARN the signing principal holds no resource-based policy for must answer HTTP 403 with x-amzn-errortype 'AccessDeniedException' and $.message == 'User: <signing principal ARN> is not authorized to perform: kafka:UpdateConnectivity on resource: <the ClusterArn sent> because no resource-based policy allows the kafka:UpdateConnectivity action'. It must NOT answer 404/NotFoundException, and must not reveal whether the cluster exists.
3. Cause 2: replay kafka-replicator-validation-errors#task#3 — CreateReplicator whose kafkaClusters[].vpcConfig.subnetIds list is outside MSK's allowed count must answer HTTP 400 with x-amzn-errortype 'BadRequestException', $.invalidParameter == 'subnetIds' and $.message == 'Too many or too few subnet ids specified.' (fixed text, nothing interpolated), and no replicator may be stored — confirm with ListReplicators that the requested replicatorName is absent.
4. Re-replay BOTH bindings — kafka-cluster-mutations-notfound-2#task#2 and kafka-replicator-validation-errors#task#3 — and both must pass before submitting.
