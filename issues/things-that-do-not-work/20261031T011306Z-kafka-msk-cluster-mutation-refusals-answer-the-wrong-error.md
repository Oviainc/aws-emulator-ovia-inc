Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-capacity-scaling-malformed-arn
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-capacity-scaling-malformed-arn kafka-cluster-mutations-notfound-2
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|UpdateBrokerCount|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateBrokerCount|4xx->4xx|$.message
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$.message
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|status
Issue-Commit: 64b86d5e14a7a25edd6b553817d01f64764c1ea9

# MSK cluster-mutation refusals answer the wrong error class and message wording

> _Note: this issue was identified against the code base at commit `64b86d5e14a7a25edd6b553817d01f64764c1ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateBrokerCount

## Description
TWO distinct root causes in the kafka (Amazon MSK) emulator, both on the refusal path of a cluster-mutating operation: the emulator decides the wrong refusal for the request and writes its own prose where AWS emits a fixed, modelled payload. Each lives in a different operation handler and must be closed separately; both must be closed before this issue is submitted.

Cause 1 of 2: UpdateBrokerCount with a malformed cluster ARN returns emulator-composed prose and a PascalCase parameter name.
AWS: UpdateBrokerCount (PUT /v1/clusters/{ClusterArn}/nodes/count) models kafka#BadRequestException with httpError 400 (Smithy: UpdateBrokerCount errors = BadRequestException/400, ForbiddenException/403, UnauthorizedException/401, InternalServerErrorException/500, ServiceUnavailableException/503; ClusterArn is an httpLabel, required). BadRequestException carries members invalidParameter and message. For a ClusterArn that is not shaped like an MSK cluster ARN the recording shows HTTP 400, x-amzn-errortype BadRequestException, invalidParameter = "clusterArn" (the modelled camelCase wire name of the offending parameter) and message = "One or more of the parameters are not valid." The whole of that sentence is FIXED text: it has no variable part, and in particular the offending ARN is NOT echoed into it. The only request-derived field in the payload is invalidParameter, and its value is the parameter NAME, not the parameter's value.
Emulator: HTTP 400 and x-amzn-errortype BadRequestException (both correct) but invalidParameter = "ClusterArn" (PascalCase, the Smithy member name rather than its wire name) and message = "<the ARN the caller sent> is not a valid Amazon MSK cluster ARN." — a self-composed sentence that both diverges from AWS's wording and leaks the caller's input into a field AWS keeps constant across requests.
Why: status and code are right, so an SDK still raises BadRequestException, but a client that classifies, greps or asserts on MSK refusal text stops matching, and an operator is shown a sentence AWS never emits. Because the emulator interpolates the request value, the message also varies run to run where AWS's is byte-identical, so any golden-file or log-assertion on it breaks.
Fix: the UpdateBrokerCount handler's malformed-cluster-ARN rejection must emit the service's shared BadRequestException refusal (fixed string, camelCase parameter token) instead of formatting its own sentence; this is the same helper the already-open issue 20261029T202809Z-kafka-msk-malformed-cluster-arn-refusals-replace-the-fixed.md asks for. That issue bundles ten sibling operations (ListNodes, UpdateBrokerStorage, UpdateBrokerType, DeleteCluster, UpdateSecurity, UpdateStorage, GetClusterPolicy, DeleteClusterPolicy, UpdateClusterKafkaVersion, ListClusterOperations) but does NOT carry the UpdateBrokerCount fixture step and does not list UpdateBrokerCount among its causes, so this refusal path is uncovered and is filed here.
Traces: kafka-cluster-capacity-scaling-malformed-arn#task#1 (UpdateBrokerCount). Handler: the UpdateBrokerCount operation handler in services/service-com-amazonaws-kafka-kafka/src/ops/ (sibling of the update_broker_storage handler cited by the related issue).

Cause 2 of 2: UpdateConnectivity on a cluster that does not exist is refused as NotFound instead of as an authorization failure.
AWS: recorded behavior only for the status choice — the Smithy model binds BOTH ForbiddenException (httpError 403) and NotFoundException (httpError 404) to UpdateConnectivity, so the model alone does not pick one, and the recorded response is the authority. The recording shows HTTP 403 with x-amzn-errortype AccessDeniedException: MSK evaluates authorization for kafka:UpdateConnectivity BEFORE it resolves the cluster, so a caller with no permission for the action is refused 403 whether or not the target cluster exists, and existence is never disclosed. The message follows AWS's standard access-denied template: the fixed text "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the ", " action", with the variable parts taken from the request and the caller's identity — the signing principal's ARN, the action name "kafka:UpdateConnectivity" (twice), and the target cluster ARN from the request's ClusterArn label. The principal ARN, account id and cluster ARN in any one recording are values filling that template, not part of it.
Emulator: HTTP 404 with x-amzn-errortype NotFoundException and message "The requested resource doesn’t exist." — it resolves the cluster first and reports absence, so it both answers the wrong status class and discloses that the cluster is missing.
Why: the status class is wrong, so an SDK raises NotFoundException where AWS raises an access-denied error. Client code that retries or branches on 403 versus 404 (for example treating 404 as "create it" and 403 as "escalate to an operator") takes the opposite branch against the emulator, and IAM/resource-policy negative tests that assert a denial pass vacuously. It also inverts MSK's non-disclosure behavior on an unauthorized request.
Fix: the UpdateConnectivity handler must run the authorization decision for kafka:UpdateConnectivity before cluster lookup and, when it denies, emit the 403 access-denied refusal (AccessDeniedException error type plus the templated message) rather than falling through to the state layer's not-found path. Note this is specific to the unauthorized case: the separately-owned fixture kafka-update-connectivity-server-enforced#task#1 shows UpdateConnectivity legitimately answering 404 when the caller IS permitted, so the not-found path must be kept for that case.
Traces: kafka-cluster-mutations-notfound-2#task#2 (UpdateConnectivity). Handler: the UpdateConnectivity operation handler in services/service-com-amazonaws-kafka-kafka/src/ops/ and the service's authorization gate.

## Evidence
Corpus: simparity-wire-aws-v1. Both clusters are verdict=mismatch and neither is owned by an open issue nor blocked by a stub (owned_by = [], stub_path = "").

Cause 1 — UpdateBrokerCount, fixture kafka-cluster-capacity-scaling-malformed-arn step task#1, expected_status 400, actual_status 400:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'
  $.message: expected 'One or more of the parameters are not valid.'
              got 'not-a-cluster-arn is not a valid Amazon MSK cluster ARN.'
  Fixed vs variable in the required payload: the entire message sentence is fixed text with NO variable part; invalidParameter is request-derived only in that it names the offending member, and its required value is the wire name 'clusterArn'. The string 'not-a-cluster-arn' is the fixture's input value and must not appear in the response at all.
  Smithy cited: UpdateBrokerCount http PUT /v1/clusters/{ClusterArn}/nodes/count, errors include BadRequestException (errorKind client, httpError 400); ClusterArn is httpLabel + required.
  Note: steps task#2 (UpdateBrokerStorage) and task#3 (UpdateBrokerType) of this SAME fixture diverge identically and are already owned by 20261029T202809Z-kafka-msk-malformed-cluster-arn-refusals-replace-the-fixed.md; task#1 / UpdateBrokerCount is absent from that issue's causes and trace ids, which is why it is reported here.

Cause 2 — UpdateConnectivity, fixture kafka-cluster-mutations-notfound-2 step task#2, expected_status 403, actual_status 404:
  status: expected 403, got 404
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'NotFoundException'
  $.message: expected the access-denied template 'User: <signing principal ARN> is not authorized to perform: kafka:UpdateConnectivity on resource: <ClusterArn from the request> because no resource-based policy allows the kafka:UpdateConnectivity action'
              got 'The requested resource doesn’t exist.'
  In the recording the template's variable slots are filled with the emulator principal ARN and the fixture's cluster ARN; those values are per-environment and only the fixed connective text plus the action name and the request's ClusterArn are the requirement.
  Smithy cited: UpdateConnectivity http PUT /v1/clusters/{ClusterArn}/connectivity, errors include ForbiddenException (httpError 403) and NotFoundException (httpError 404) — the model permits both, the recording selects 403; the wire error type in the recording is AccessDeniedException.

## Steps to Reproduce
1. Boot the emulator from this checkout (the data listener the replay used answered on /__admin/health).
2. Cause 1: replay fixture kafka-cluster-capacity-scaling-malformed-arn step task#1 — UpdateBrokerCount (PUT /v1/clusters/not-a-cluster-arn/nodes/count) with a malformed ClusterArn. Assert HTTP 400, x-amzn-errortype BadRequestException, and body exactly {"invalidParameter":"clusterArn","message":"One or more of the parameters are not valid."}. Assert the ARN the caller sent does not appear anywhere in the response body.
3. Repeat step 2 with a different malformed ARN string and assert the two message fields are byte-identical, proving the message is constant and not request-derived.
4. Cause 2: replay fixture kafka-cluster-mutations-notfound-2 step task#2 — UpdateConnectivity against a cluster ARN that does not exist in emulator state. Assert HTTP 403, x-amzn-errortype AccessDeniedException, and a message matching the access-denied template 'User: <principal> is not authorized to perform: kafka:UpdateConnectivity on resource: <the ClusterArn sent> because no resource-based policy allows the kafka:UpdateConnectivity action'. Assert the response is NOT 404/NotFoundException and does not reveal that the cluster is absent.
5. Regression-guard Cause 2: re-replay kafka-update-connectivity-server-enforced#task#1, which must still answer 404 for its permitted caller, so the authorization gate has not simply replaced the not-found path.
6. Re-replay both bundled bindings: kafka-cluster-capacity-scaling-malformed-arn#task#1 and kafka-cluster-mutations-notfound-2#task#2.
