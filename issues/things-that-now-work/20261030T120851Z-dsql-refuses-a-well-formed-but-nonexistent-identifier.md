Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-missing-cluster-error-surface
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-missing-cluster-error-surface dsql-cluster-subresource-guards
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|UpdateCluster|4xx->4xx|$.message
Issue-Key: dsql|UpdateCluster|4xx->4xx|$.resourceId
Issue-Key: dsql|UpdateCluster|4xx->4xx|$.resourceType
Issue-Key: dsql|UpdateCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|UpdateCluster|4xx->4xx|status
Issue-Key: dsql|GetVpcEndpointServiceName|4xx->4xx|$.message
Issue-Key: dsql|GetVpcEndpointServiceName|4xx->4xx|$.resourceId
Issue-Key: dsql|GetVpcEndpointServiceName|4xx->4xx|$.resourceType
Issue-Key: dsql|GetVpcEndpointServiceName|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|GetVpcEndpointServiceName|4xx->4xx|status
Issue-Key: dsql|PutClusterPolicy|4xx->4xx|$.resourceId
Issue-Key: dsql|PutClusterPolicy|4xx->4xx|$.resourceType
Issue-Key: dsql|PutClusterPolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|PutClusterPolicy|4xx->4xx|status
Issue-Key: dsql|DeleteClusterPolicy|4xx->4xx|$.message
Issue-Key: dsql|DeleteClusterPolicy|4xx->4xx|$.resourceId
Issue-Key: dsql|DeleteClusterPolicy|4xx->4xx|$.resourceType
Issue-Key: dsql|DeleteClusterPolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|DeleteClusterPolicy|4xx->4xx|status
Issue-Key: dsql|ListStreams|4xx->4xx|$.message
Issue-Key: dsql|ListStreams|4xx->4xx|$.resourceId
Issue-Key: dsql|ListStreams|4xx->4xx|$.resourceType
Issue-Key: dsql|ListStreams|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|ListStreams|4xx->4xx|status
Issue-Key: dsql|GetStream|4xx->4xx|$.message
Issue-Key: dsql|GetStream|4xx->4xx|$.resourceId
Issue-Key: dsql|GetStream|4xx->4xx|$.resourceType
Issue-Key: dsql|GetStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|GetStream|4xx->4xx|status
Issue-Key: dsql|DeleteStream|4xx->4xx|$.message
Issue-Key: dsql|DeleteStream|4xx->4xx|$.resourceId
Issue-Key: dsql|DeleteStream|4xx->4xx|$.resourceType
Issue-Key: dsql|DeleteStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|DeleteStream|4xx->4xx|status
Issue-Related: 20261029T193445Z-dsql-refusal-messages-are-emulator-authored-instead-of.md
Issue-Commit: e72ab6d9ecc28f469f99eceb42a3c20602b3ead9

# dsql refuses a well-formed but nonexistent identifier as ValidationException 400 instead of ResourceNotFoundException 404

> _Note: this issue was identified against the code base at commit `e72ab6d9ecc28f469f99eceb42a3c20602b3ead9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCluster

## Description
SEVEN causes, one shared fault. Every DSQL operation that addresses a cluster or a stream by identifier in its URI runs a pre-lookup FORM check that rejects identifiers the model declares legal. `ClusterId` and `StreamId` are both `@pattern ^[a-z0-9]{26}$` (Smithy: GetStream.clusterIdentifier/streamIdentifier, UpdateCluster.identifier, GetVpcEndpointServiceName.identifier, PutClusterPolicy.identifier, DeleteClusterPolicy.identifier, ListStreams.clusterIdentifier). A 26-character lowercase-alphanumeric identifier therefore SATISFIES the model and is simply an identifier this account does not hold, so the request must reach the store and be refused as absent. The emulator instead refuses it as malformed before any lookup, so the whole not-found surface of the service is unreachable for any identifier its minting scheme would not itself have produced.

Shared AWS behavior for all seven: every one of these operations models `ResourceNotFoundException` with `httpError 404` and REQUIRED members `message`, `resourceId` and `resourceType` (Smithy: ResourceNotFoundException, "The resource could not be found"; resourceId "The resource ID could not be found", resourceType "The resource type could not be found"). The reply is HTTP 404 with `x-amzn-errortype: ResourceNotFoundException` and a body carrying all three members. The message's FIXED text is "The resource with ARN " + "<arn>" + " doesn't exist. Verify the ARN and try again."; the only VARIABLE part is the ARN, built from the request and state as the modeled ARN patterns require — `arn:<partition>:dsql:<region>:<account>:cluster/<clusterId>` for a cluster and `arn:<partition>:dsql:<region>:<account>:cluster/<clusterId>/stream/<streamId>` for a stream. `resourceId` is the addressed identifier taken verbatim from the request; `resourceType` is the literal "cluster" or "stream". The recorded account, region and identifier values in the fixtures fill that template and are not part of it.

Shared emulator behavior: HTTP 400 with `x-amzn-errortype: ValidationException` and an emulator-authored "format is invalid" sentence, with `resourceId` and `resourceType` absent entirely.

Shared why: a client cannot distinguish "this resource does not exist" from "you sent me garbage". Every SDK's not-found handling keys off the 404/`ResourceNotFoundException` pair — waiters that poll until a delete is observed, idempotent create-if-absent, and the `resourceId`/`resourceType` fields callers log or branch on. Against the emulator all of those see a non-retryable client validation failure instead, and the error is also a lie about the request: the identifier it calls malformed is exactly what the model declares well formed.

Shared fix: the identifier FORM gate that each handler runs before touching the store. It must accept everything `^[a-z0-9]{26}$` admits and reject only what the model's pattern excludes (wrong length, uppercase, or any character outside [a-z0-9]); anything that passes must fall through to the store lookup and, when absent, to that operation's `ResourceNotFoundException` constructor. Hypothesis (neither the model nor the recordings state this): the gate is validating the identifier as a decodable ULID rather than against the modeled pattern, which is why an all-'a' identifier — legal under the pattern, but with no valid ULID timestamp prefix — is turned away.

Cause 1 of 7: UpdateCluster, a cluster identifier that is well formed but absent. AWS: 404 ResourceNotFoundException, message "The resource with ARN <clusterArn> doesn't exist. Verify the ARN and try again.", resourceId = the `identifier` sent, resourceType = "cluster" (Smithy: UpdateCluster errors ResourceNotFoundException httpError 404; identifier @pattern ^[a-z0-9]{26}$). Emulator: 400 ValidationException, message "Invalid input 'identifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again." — its fixed text is "Invalid input '", "identifier: ", "'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again." with the identifier interpolated — and no resourceId/resourceType. Why: an update against a deleted cluster is reported as a bad request, so retry/reconcile logic that branches on not-found never fires. Fix: the pre-lookup form gate in UpdateCluster's handler, then its not-found path. Traces: dsql-missing-cluster-error-surface#task#1.

Cause 2 of 7: GetVpcEndpointServiceName, same input class. AWS: 404 ResourceNotFoundException with the cluster ARN message, resourceId = the `identifier` sent, resourceType = "cluster" (Smithy: GetVpcEndpointServiceName errors ResourceNotFoundException httpError 404). Emulator: 400 ValidationException with the same "The cluster identifier format is invalid" sentence and no resourceId/resourceType. Why: a caller resolving the endpoint service for a cluster that has gone away cannot tell the cluster is gone. Fix: the form gate in GetVpcEndpointServiceName's handler. Traces: dsql-missing-cluster-error-surface#task#2.

Cause 3 of 7: PutClusterPolicy against an absent cluster. AWS: 404 ResourceNotFoundException with the cluster ARN message, resourceId = `identifier`, resourceType = "cluster" (Smithy: PutClusterPolicy errors ResourceNotFoundException httpError 404). Emulator: 400 ValidationException, "The cluster identifier format is invalid" sentence, no resourceId/resourceType. Why: policy reconciliation treats a vanished cluster as an invalid request and aborts rather than skipping it. Fix: the form gate in PutClusterPolicy's handler, ahead of its policy-document validation. Traces: dsql-missing-cluster-error-surface#task#3.

Cause 4 of 7: DeleteClusterPolicy against an absent cluster. AWS: 404 ResourceNotFoundException with the cluster ARN message, resourceId = `identifier`, resourceType = "cluster" (Smithy: DeleteClusterPolicy errors ResourceNotFoundException httpError 404). Emulator: 400 ValidationException, "The cluster identifier format is invalid" sentence, no resourceId/resourceType. Why: teardown paths that tolerate an already-deleted cluster by catching not-found instead fail hard. Fix: the form gate in DeleteClusterPolicy's handler. Traces: dsql-missing-cluster-error-surface#task#4.

Cause 5 of 7: ListStreams for an absent cluster. AWS: 404 ResourceNotFoundException with the cluster ARN message, resourceId = `clusterIdentifier`, resourceType = "cluster" (Smithy: ListStreams errors ResourceNotFoundException httpError 404; clusterIdentifier @pattern ^[a-z0-9]{26}$). Emulator: 400 ValidationException whose message is the same template keyed on the other member name — "Invalid input 'clusterIdentifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again." — and no resourceId/resourceType. Why: a paginating client enumerating streams of a cluster cannot distinguish a removed cluster from a rejected request. Fix: the form gate in ListStreams' handler. Traces: dsql-missing-cluster-error-surface#task#5.

Cause 6 of 7: GetStream for a well-formed but absent STREAM of a cluster that DOES exist. AWS: 404 ResourceNotFoundException, message "The resource with ARN <streamArn> doesn't exist. Verify the ARN and try again." over the nested stream ARN, resourceId = the `streamIdentifier` sent, resourceType = "stream" (Smithy: GetStream errors ResourceNotFoundException httpError 404; streamIdentifier @pattern ^[a-z0-9]{26}$). Emulator: 400 ValidationException, message "Aurora DSQL can't process the request because the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again." — fixed text with the member name interpolated — and no resourceId/resourceType. Why: the stream not-found surface is unreachable, so a caller polling a stream it just deleted gets a validation failure forever. Note: the open issue 20261028T121757Z-dsql-getstream-error-response.md concerns which REGION that 404's ARN is built from; it does not cover this cause, because here the 404 is never produced at all. Fix: the stream-identifier form gate in GetStream's handler (`services/service-com-amazonaws-dsql-dsql/src/ops/get_stream.rs:168-173`, the `validate_stream_identifier` call that precedes the store lookup at :189). Traces: dsql-cluster-subresource-guards#task#4, dsql-missing-cluster-error-surface#task#6.

Cause 7 of 7: DeleteStream for a well-formed but absent stream of an existing cluster. AWS: 404 ResourceNotFoundException with the nested stream ARN message, resourceId = `streamIdentifier`, resourceType = "stream" (Smithy: DeleteStream errors ResourceNotFoundException httpError 404). Emulator: 400 ValidationException with the same "base32-encoded 26-character string" sentence and no resourceId/resourceType. Why: an idempotent delete that swallows not-found instead surfaces a hard client error on the second call. Fix: the stream-identifier form gate in DeleteStream's handler, which duplicates GetStream's. Traces: dsql-cluster-subresource-guards#task#5, dsql-missing-cluster-error-surface#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (12 fixtures, 96 steps, 32 mismatched). All seven clusters below report the same divergence shape: expected 404, got 400; `$.message` value_mismatch; `$.resourceId` and `$.resourceType` missing; `$headers.x-amzn-errortype` shape_mismatch, got 'ValidationException'.

Cause 1 — dsql-missing-cluster-error-surface#task#1, UpdateCluster:
  status: expected 404, got 400
  $.message: expected "The resource with ARN arn:aws:dsql:us-west-2:000000000000:cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa doesn't exist. Verify the ARN and try again.", got "Invalid input 'identifier: aaaaaaaaaaaaaaaaaaaaaaaaaa'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again."
  $.resourceId: missing (expected 'aaaaaaaaaaaaaaaaaaaaaaaaaa'); $.resourceType: missing (expected 'cluster')
  $headers.x-amzn-errortype: got 'ValidationException', expected the modeled ResourceNotFoundException

Cause 2 — dsql-missing-cluster-error-surface#task#2, GetVpcEndpointServiceName: identical expected/actual pair to Cause 1.
Cause 3 — dsql-missing-cluster-error-surface#task#3, PutClusterPolicy: identical expected/actual pair to Cause 1.
Cause 4 — dsql-missing-cluster-error-surface#task#4, DeleteClusterPolicy: identical expected/actual pair to Cause 1.
Cause 5 — dsql-missing-cluster-error-surface#task#5, ListStreams: same, except the emulator's message names the other member: got "Invalid input 'clusterIdentifier: aaaaaaaaaaaaaaaaaaaaaaaaaa'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again."

Cause 6 — dsql-cluster-subresource-guards#task#4, GetStream (cluster exists, stream does not; also affects dsql-missing-cluster-error-surface#task#6):
  status: expected 404, got 400
  $.message: expected "The resource with ARN arn:aws:dsql:us-west-2:000000000000:cluster/01m66b8n8dx7mygewcdaz59tjz/stream/aaaaaaaaaaaaaaaaaaaaaaaaaa doesn't exist. Verify the ARN and try again.", got "Aurora DSQL can't process the request because the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again."
  $.resourceId: missing (expected 'aaaaaaaaaaaaaaaaaaaaaaaaaa'); $.resourceType: missing (expected 'stream'); $headers.x-amzn-errortype: got 'ValidationException'

Cause 7 — dsql-cluster-subresource-guards#task#5, DeleteStream (also affects dsql-missing-cluster-error-surface#task#7): identical expected/actual pair to Cause 6.

Why the rejected identifier is legal: the fixtures address `aaaaaaaaaaaaaaaaaaaaaaaaaa` — 26 characters, all in [a-z0-9] — which satisfies the modeled `@pattern ^[a-z0-9]{26}$` on ClusterId and StreamId for every one of these seven operations, so no ValidationException is admissible for its form. The pre-lookup gate is visible in the handler: services/service-com-amazonaws-dsql-dsql/src/ops/get_stream.rs:158-173 comments "The FORM of both identifiers is decided before any lookup" and returns ValidationException from `validate_cluster_identifier` / `validate_stream_identifier` before the store is read at :175-189, where the ResourceNotFoundException path lives.

(Values such as the account id 000000000000, region us-west-2, and the cluster id 01m66b8n8dx7mygewcdaz59tjz are this recording's fillers for the ARN template, not requirements.)

## Steps to Reproduce
1. Build the emulator from this checkout and boot it so the dsql endpoint answers.
2. Cause 1: POST /cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa (UpdateCluster) with a body such as {"deletionProtectionEnabled": false}. The identifier is 26 chars of [a-z0-9], so it satisfies ClusterId's modeled pattern, and no such cluster exists. Observe 400 with x-amzn-errortype: ValidationException and a "cluster identifier format is invalid" message, instead of 404 ResourceNotFoundException carrying message/resourceId/resourceType.
3. Causes 2-5: repeat against the same identifier on GET /clusters/aaaaaaaaaaaaaaaaaaaaaaaaaa/vpc-endpoint-service-name, POST /cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa/policy (with any valid policy document), DELETE /cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa/policy, and GET /stream/aaaaaaaaaaaaaaaaaaaaaaaaaa. Each answers 400 ValidationException instead of 404 ResourceNotFoundException; ListStreams' message names clusterIdentifier rather than identifier.
4. Causes 6-7: CreateCluster and let it settle to ACTIVE, note its identifier <c>. Then GET /stream/<c>/aaaaaaaaaaaaaaaaaaaaaaaaaa and DELETE /stream/<c>/aaaaaaaaaaaaaaaaaaaaaaaaaa. The cluster exists and the stream identifier matches StreamId's modeled pattern, so AWS answers 404 ResourceNotFoundException over the nested stream ARN with resourceType 'stream'; the emulator answers 400 ValidationException claiming the streamIdentifier is not a valid base32-encoded 26-character string.
5. Re-replay to confirm: fixture dsql-missing-cluster-error-surface steps task#1 through task#7, and fixture dsql-cluster-subresource-guards steps task#4 and task#5.
