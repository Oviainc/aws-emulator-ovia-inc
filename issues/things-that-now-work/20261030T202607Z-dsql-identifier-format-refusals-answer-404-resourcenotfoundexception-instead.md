Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-cluster-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-cluster-error-paths dsql-request-validation-error-surface dsql-cdc-stream-error-handling
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|GetCluster|4xx->4xx|$.message
Issue-Key: dsql|GetCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|GetCluster|4xx->4xx|status
Issue-Key: dsql|DeleteCluster|4xx->4xx|$.message
Issue-Key: dsql|DeleteCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|DeleteCluster|4xx->4xx|status
Issue-Key: dsql|GetClusterPolicy|4xx->4xx|$.message
Issue-Key: dsql|GetClusterPolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|GetClusterPolicy|4xx->4xx|status
Issue-Key: dsql|GetStream|4xx->4xx|$.message
Issue-Key: dsql|GetStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|GetStream|4xx->4xx|status
Issue-Key: dsql|DeleteStream|4xx->4xx|$.message
Issue-Key: dsql|DeleteStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|DeleteStream|4xx->4xx|status
Issue-Commit: 57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf

# dsql identifier-format refusals answer 404 ResourceNotFoundException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
Five path-identifier refusals are answered from the wrong error class. Aurora DSQL validates an httpLabel identifier's FORMAT before it ever looks the resource up: an identifier that is 26 lowercase-alphanumeric characters but is not a valid base32-encoded (Crockford/ULID) identifier is refused up-front with 400 ValidationException. The emulator has no such format gate: it passes any 26-char lowercase-alphanumeric label straight into the store, misses, and refuses with 404 ResourceNotFoundException. The status class, the x-amzn-errortype error code AND the message are all wrong, so a client cannot distinguish "you sent a malformed id" from "that resource is gone". This is a status/error-code defect, distinct from the open issue 20261029T193445Z-dsql-refusal-messages-are-emulator-authored-instead-of, which covers only the wording of refusals whose status already matches.

Cause 1 of 5: GetCluster with a cluster identifier that matches the member pattern but is not a valid base32 identifier.
AWS: 400 ValidationException, header x-amzn-errortype: ValidationException, message "Invalid input 'identifier: <identifier>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again." — fixed text is "Invalid input 'identifier: ", then "'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again."; the only variable part is the rejected identifier value taken verbatim from the request URI label (the recording's zzzzzzzzzzzzzzzzzzzzzzzzzz is one instance of that value, not part of the template). Smithy: GetCluster input identifier targets ClusterId with @pattern ^[a-z0-9]{26}$ and @httpLabel, http GET /cluster/{identifier}; modeled errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). The model's pattern alone does not reject this value, so the base32 requirement is recorded behavior only: the recording shows AWS refusing format before lookup.
Emulator: 404 with x-amzn-errortype: ResourceNotFoundException and message "The resource with ARN <cluster arn> doesn't exist. Verify the ARN and try again."
Why: a client that branches on 400/ValidationException to report a bad caller-supplied id, and on 404/ResourceNotFoundException to treat the cluster as deleted, takes the wrong branch — malformed input is reported as a vanished cluster, so retry/reconcile logic deletes or re-creates state instead of surfacing a user error.
Fix: GetCluster's input validation (the shared cluster-identifier format check that must run before the state lookup in the handler).
Traces: dsql-cluster-error-paths#task#1.

Cause 2 of 5: DeleteCluster with the same class of malformed cluster identifier.
AWS: 400 ValidationException, x-amzn-errortype: ValidationException, the same fixed sentence as Cause 1 with the request's identifier interpolated. Smithy: DeleteCluster input identifier targets ClusterId (@pattern ^[a-z0-9]{26}$, @httpLabel), http DELETE /cluster/{identifier}, errors include ValidationException 400 and ResourceNotFoundException 404.
Emulator: 404 ResourceNotFoundException with the "resource with ARN ... doesn't exist" sentence.
Why: as Cause 1, on a mutating path — a delete of a malformed id reads as an already-deleted cluster, so idempotent-delete logic treats the call as successful cleanup.
Fix: same shared cluster-identifier format validation, reached from DeleteCluster's handler before lookup.
Traces: dsql-cluster-error-paths#task#2.

Cause 3 of 5: GetClusterPolicy with the same class of malformed cluster identifier.
AWS: 400 ValidationException with the same fixed cluster-identifier sentence, the request's identifier interpolated. Smithy: GetClusterPolicy input identifier targets ClusterId (@pattern ^[a-z0-9]{26}$, @httpLabel), http GET /cluster/{identifier}/policy, errors include ValidationException 400 and ResourceNotFoundException 404.
Emulator: 404 ResourceNotFoundException with the ARN sentence.
Why: as Cause 1, on the sub-resource read path; the policy reader cannot tell a bad identifier from a cluster without a policy.
Fix: same shared cluster-identifier format validation, applied in the policy-read handler before the lookup.
Traces: dsql-request-validation-error-surface#task#5.

Cause 4 of 5: GetStream with a streamIdentifier that is 26 lowercase-alphanumeric characters but not a valid base32 string.
AWS: 400 ValidationException whose message is the fixed sentence "Aurora DSQL can't process the request because the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again." — the whole sentence is fixed text; the variable part is the offending member name (streamIdentifier) and the format description it states, and the rejected value itself is NOT echoed. Smithy: GetStream input streamIdentifier targets StreamId (@pattern ^[a-z0-9]{26}$, @httpLabel), http GET /stream/{clusterIdentifier}/{streamIdentifier}, errors include ValidationException 400 and ResourceNotFoundException 404; the base32 rule itself is recorded behavior only.
Emulator: 404 with x-amzn-errortype: ResourceNotFoundException and "The resource with ARN <stream arn> doesn't exist. Verify the ARN and try again."
Why: as Cause 1, for streams — a malformed stream id is reported as a deleted stream, and the error carries no indication that the identifier format is the problem.
Fix: GetStream's input validation (stream-identifier format check in the handler, before the stream lookup).
Traces: dsql-cdc-stream-error-handling#task#5.

Cause 5 of 5: DeleteStream with the same class of malformed streamIdentifier.
AWS: 400 ValidationException with the same fixed streamIdentifier sentence as Cause 4. Smithy: DeleteStream input streamIdentifier targets StreamId (@pattern ^[a-z0-9]{26}$, @httpLabel), http DELETE /stream/{clusterIdentifier}/{streamIdentifier}, errors include ValidationException 400 and ResourceNotFoundException 404.
Emulator: 404 ResourceNotFoundException with the ARN sentence.
Why: as Cause 4, on the mutating path — a delete against a malformed stream id looks like a completed deletion.
Fix: same stream-identifier format validation, reached from DeleteStream's handler before lookup.
Traces: dsql-cdc-stream-error-handling#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 70.8%, 28 mismatched steps):
- dsql-cluster-error-paths#task#1 GetCluster: status expected 400, got 404; $headers.x-amzn-errortype expected 'ValidationException', got 'ResourceNotFoundException'; $.message expected "Invalid input 'identifier: zzzzzzzzzzzzzzzzzzzzzzzzzz'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again.", got "The resource with ARN arn:aws:dsql:us-west-2:000000000000:cluster/zzzzzzzzzzzzzzzzzzzzzzzzzz doesn't exist. Verify the ARN and try again."
- dsql-cluster-error-paths#task#2 DeleteCluster: identical expected/actual pair (400/ValidationException vs 404/ResourceNotFoundException).
- dsql-request-validation-error-surface#task#5 GetClusterPolicy: status expected 400, got 404; $.message expected "Invalid input 'identifier: bbbbbbbbbbbbbbbbbbbbbbbbbb'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again.", got the ARN-doesn't-exist sentence; errortype 'ResourceNotFoundException'.
- dsql-cdc-stream-error-handling#task#5 GetStream: status expected 400, got 404; $.message expected "Aurora DSQL can't process the request because the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again.", got "The resource with ARN arn:aws:dsql:us-west-2:000000000000:cluster/01m677rakr33ytpwn0zsx7b8en/stream/abcdefghijklmnopqrstuvwxyz doesn't exist. Verify the ARN and try again."; errortype 'ResourceNotFoundException'.
- dsql-cdc-stream-error-handling#task#6 DeleteStream: identical expected/actual pair to task#5.
None of these five clusters was reported as stub-blocked or owned by an open issue.

## Steps to Reproduce
1. Replay dsql-cluster-error-paths step task#1: GET /cluster/{identifier} with a 26-char lowercase-alphanumeric identifier that is not a valid base32 identifier -> expect 400 ValidationException with the cluster-identifier-format sentence; emulator answers 404 ResourceNotFoundException.
2. Replay dsql-cluster-error-paths step task#2: DELETE /cluster/{identifier} with the same identifier -> compare status, x-amzn-errortype and $.message.
3. Replay dsql-request-validation-error-surface step task#5: GET /cluster/{identifier}/policy with such an identifier -> compare status, x-amzn-errortype and $.message.
4. Replay dsql-cdc-stream-error-handling step task#5: GET /stream/{clusterIdentifier}/{streamIdentifier} with a valid cluster id and a non-base32 26-char streamIdentifier -> expect 400 ValidationException with the streamIdentifier-format sentence.
5. Replay dsql-cdc-stream-error-handling step task#6: DELETE the same stream path -> same comparison.
