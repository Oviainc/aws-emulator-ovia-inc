Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-cluster-subresource-guards
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-cluster-subresource-guards dsql-missing-cluster-error-surface
Issue-Shape-Id: com.amazonaws.dsql#DSQL
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
Issue-Related: 20261029T193445Z-dsql-refusal-messages-are-emulator-authored-instead-of.md
Issue-Commit: 74acd3b5911f8cc01604f18ec0c28de145582648

# dsql rejects syntactically valid 26-character identifiers as ValidationException instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `74acd3b5911f8cc01604f18ec0c28de145582648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStream

## Description
Seven dsql operations share one fault: an identifier that SATISFIES the modeled identifier pattern but names no stored resource is refused as a malformed input (ValidationException, HTTP 400) instead of as an absent resource (ResourceNotFoundException, HTTP 404). Smithy: ClusterId and StreamId both carry @pattern ^[a-z0-9]{26}$, so any 26-character lowercase-alphanumeric string is a well-formed identifier; every one of these operations models ResourceNotFoundException (httpError 404) alongside ValidationException (httpError 400), and ResourceNotFoundException requires three members (message, resourceId, resourceType). Hypothesis: the emulator's identifier guard does not implement the modeled pattern but a stricter decode (it reports a 'valid base32-encoded 26-character string' requirement for streams and an 'identifier format is invalid' refusal for clusters), so identifiers whose characters decode outside the range it expects — e.g. a 26-character all-lowercase-letter identifier — are rejected before any store lookup happens. There are 7 causes to close; each is listed below with its own fixture and step.

Cause 1 of 7 — GetStream (fixture dsql-cluster-subresource-guards, step task#4). AWS: for a cluster that exists and a well-formed streamIdentifier it does not hold, GetStream returns 404 ResourceNotFoundException with x-amzn-errortype naming that error, message = the fixed sentence "The resource with ARN <arn> doesn't exist. Verify the ARN and try again." where <arn> is the nested stream ARN of the addressed resource (fixed text: "The resource with ARN ", " doesn't exist. Verify the ARN and try again."; variable part: the ARN, derived from partition/region/account of the stored cluster plus cluster/stream identifiers from the request), resourceId = the streamIdentifier from the request, resourceType = "stream". Emulator: 400 with x-amzn-errortype ValidationException, message "Aurora DSQL can't process the request because the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again.", and no resourceId/resourceType. Why: a client that distinguishes 'stream gone' from 'bad request' — a poller, a delete-if-exists, a retry wrapper — sees a non-retryable input error for a resource that is merely absent, and loses resourceId/resourceType entirely. Fix: GetStream's input validation must enforce only the modeled pattern and defer absence to the store lookup in the handler, which already has a ResourceNotFoundException path.

Cause 2 of 7 — DeleteStream (fixture dsql-cluster-subresource-guards, step task#5). AWS: identical refusal to Cause 1 — 404 ResourceNotFoundException, same message template over the nested stream ARN, resourceId = the requested streamIdentifier, resourceType = "stream". Emulator: 400 ValidationException with the same base32-format message and no resourceId/resourceType. Why: a delete that should be idempotent-on-absent reports a client input error, so cleanup paths abort instead of treating the stream as already gone. Fix: DeleteStream's input validation / handler, same split as Cause 1.

Cause 3 of 7 — UpdateCluster (fixture dsql-missing-cluster-error-surface, step task#1). AWS: for a well-formed cluster identifier no cluster exists for, 404 ResourceNotFoundException with message "The resource with ARN <cluster-arn> doesn't exist. Verify the ARN and try again." (<cluster-arn> built from the request's partition/region/account and the identifier from the request), resourceId = the identifier, resourceType = "cluster". Emulator: 400 ValidationException, message "Invalid input 'identifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again.", no resourceId/resourceType. Why: an update against a deleted cluster is misreported as a malformed request, so a caller retries with a 'corrected' identifier that was never wrong. Fix: UpdateCluster's cluster-identifier validation must accept the modeled pattern and let the store lookup raise ResourceNotFoundException.

Cause 4 of 7 — GetVpcEndpointServiceName (fixture dsql-missing-cluster-error-surface, step task#2). AWS: 404 ResourceNotFoundException with the same cluster message template, resourceId = the identifier, resourceType = "cluster". Emulator: 400 ValidationException with the 'cluster identifier format is invalid' message and no resourceId/resourceType. Why: endpoint discovery for a cluster that no longer exists returns the wrong error class, so VPC-endpoint provisioning logic cannot branch on absence. Fix: this operation's identifier validation / handler lookup.

Cause 5 of 7 — PutClusterPolicy (fixture dsql-missing-cluster-error-surface, step task#3). AWS: 404 ResourceNotFoundException, same cluster message template, resourceId = the identifier, resourceType = "cluster". Emulator: 400 ValidationException with the 'cluster identifier format is invalid' message, no resourceId/resourceType. Why: a policy write to an absent cluster is reported as invalid input, masking the real condition from policy-reconciliation code. Fix: PutClusterPolicy's identifier validation / handler lookup.

Cause 6 of 7 — DeleteClusterPolicy (fixture dsql-missing-cluster-error-surface, step task#4). AWS: 404 ResourceNotFoundException, same cluster message template, resourceId = the identifier, resourceType = "cluster". Emulator: 400 ValidationException with the 'cluster identifier format is invalid' message, no resourceId/resourceType. Why: same as Cause 5 on the teardown side — the delete cannot be treated as a no-op on an absent cluster. Fix: DeleteClusterPolicy's identifier validation / handler lookup.

Cause 7 of 7 — ListStreams (fixture dsql-missing-cluster-error-surface, step task#5). AWS: 404 ResourceNotFoundException, same cluster message template, resourceId = the clusterIdentifier from the request, resourceType = "cluster". Emulator: 400 ValidationException, message "Invalid input 'clusterIdentifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again.", no resourceId/resourceType. Why: enumerating streams of a removed cluster yields a non-retryable input error rather than the modeled absence, so paginating callers cannot tell an empty/absent cluster from a bad call. Fix: ListStreams' clusterIdentifier validation / handler lookup.

Related open issues that do NOT cover these: 20261028T121757Z-dsql-getstream-error-response.md concerns the Region segment INSIDE a 404 that GetStream already returns for an unknown stream of a known cluster — it presupposes the 404 these causes never reach. 20261029T193445Z-dsql-refusal-messages-are-emulator-authored-instead-of.md covers refusals that already carry the correct status; these seven carry the wrong status and the wrong error shape.

## Evidence
Replay of trace set simparity-wire-aws-v1 (12 fixtures, 96 steps, 64 passed, 32 mismatched, score 66.7%). Seven divergence clusters, all service=dsql, none owned by an open issue and none blocked by a stub:

1. GetStream — dsql-cluster-subresource-guards#task#4 (also dsql-missing-cluster-error-surface#task#7): status expected 404, got 400; $headers.x-amzn-errortype shape_mismatch (got 'ValidationException'); $.resourceId missing (expected the requested stream identifier); $.resourceType missing (expected 'stream'); $.message value_mismatch — expected "The resource with ARN <cluster-arn>/stream/<stream-id> doesn't exist. Verify the ARN and try again.", got "Aurora DSQL can't process the request because the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again."
2. DeleteStream — dsql-cluster-subresource-guards#task#5 (also dsql-missing-cluster-error-surface#task#8... per replay: dsql-missing-cluster-error-surface#task#7): same four deltas as (1), same expected vs actual messages.
3. UpdateCluster — dsql-missing-cluster-error-surface#task#1: status expected 404, got 400; errortype 'ValidationException'; resourceId/resourceType missing (expected the identifier and 'cluster'); message expected "The resource with ARN <cluster-arn> doesn't exist. Verify the ARN and try again.", got "Invalid input 'identifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again."
4. GetVpcEndpointServiceName — dsql-missing-cluster-error-surface#task#2: identical deltas to (3).
5. PutClusterPolicy — dsql-missing-cluster-error-surface#task#3: identical deltas to (3).
6. DeleteClusterPolicy — dsql-missing-cluster-error-surface#task#4: identical deltas to (3).
7. ListStreams — dsql-missing-cluster-error-surface#task#5: identical deltas to (3), except the emulator names the member clusterIdentifier: "Invalid input 'clusterIdentifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again."

Model grounding (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json): ClusterId and StreamId @pattern ^[a-z0-9]{26}$ on every input member above (GetStream.clusterIdentifier/streamIdentifier, DeleteStream.clusterIdentifier/streamIdentifier, UpdateCluster.identifier, GetVpcEndpointServiceName.identifier, PutClusterPolicy.identifier, DeleteClusterPolicy.identifier, ListStreams.clusterIdentifier) — a 26-character all-lowercase identifier satisfies it. Each of the seven operations models ResourceNotFoundException httpError 404 AND ValidationException httpError 400. ResourceNotFoundException members: message (required), resourceId (required, 'The resource ID could not be found.'), resourceType (required, 'The resource type could not be found.') — the emulator emits none of the latter two on these paths.

## Steps to Reproduce
1. git clone the repo and boot the emulator; confirm /__admin/health answers 200.
2. Replay fixture dsql-cluster-subresource-guards and watch step task#4: after the fixture's cluster exists, GET /stream/<existing-cluster-id>/<26-char lowercase stream id the cluster does not hold>. Expect 404 ResourceNotFoundException with message "The resource with ARN <stream-arn> doesn't exist. Verify the ARN and try again.", resourceId = the stream id, resourceType = "stream"; observe 400 ValidationException with the "...streamIdentifier ... 'a valid base32-encoded 26-character string'..." message and no resourceId/resourceType.
3. Same fixture, step task#5: DELETE /stream/<existing-cluster-id>/<same stream id> — same wrong 400.
4. Replay fixture dsql-missing-cluster-error-surface against an empty store and watch steps task#1..task#5, which address a 26-character lowercase cluster identifier that satisfies ^[a-z0-9]{26}$ and exists nowhere: POST /cluster/{id} (UpdateCluster, task#1), GET /clusters/{id}/vpc-endpoint-service-name (task#2), POST /cluster/{id}/policy (task#3), DELETE /cluster/{id}/policy (task#4), GET /stream/{id} (ListStreams, task#5).
5. Each must answer 404 ResourceNotFoundException with message "The resource with ARN <cluster-arn> doesn't exist. Verify the ARN and try again.", resourceId = the identifier and resourceType = "cluster"; each instead answers 400 with x-amzn-errortype: ValidationException and a 'cluster identifier format is invalid' message, omitting resourceId and resourceType.
