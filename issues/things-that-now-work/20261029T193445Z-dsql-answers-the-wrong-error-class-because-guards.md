Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-cluster-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-cluster-error-paths dsql-request-validation-error-surface dsql-cdc-stream-error-handling dsql-cluster-policy-version-guards
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
Issue-Key: dsql|DeleteClusterPolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dsql|DeleteClusterPolicy|4xx->4xx|status
Issue-Commit: 16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22

# dsql answers the wrong error class because guards run before existence and format checks

> _Note: this issue was identified against the code base at commit `16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
Six operations answer a different error class than Aurora DSQL does, because the emulator goes straight to its state map (or to a version guard) instead of first validating the shape of the identifier it was handed. A client therefore sees ResourceNotFoundException/ConflictException where AWS reports ValidationException/ResourceNotFoundException.

Cause 1 of 6: GetCluster with a cluster identifier that is syntactically not a DSQL identifier.
AWS: refuses the request before any lookup with 400 ValidationException. Smithy: GetCluster.identifier targets ClusterId with pattern ^[a-z0-9]{26}$, and com.amazonaws.dsql#ValidationException requires `message` and `reason` (ValidationExceptionReason one of unknownOperation|cannotParse|fieldValidationFailed|deletionProtectionEnabled|other). The recorded response carries x-amzn-errortype: ValidationException and the message template "Invalid input 'identifier: <identifier>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again." — the two sentences after the quoted phrase are fixed text, and the only variable part is the member name and the rejected identifier value echoed from the request path label.
Emulator: 404 with x-amzn-errortype: ResourceNotFoundException and the message "The resource with ARN <cluster-arn> doesn't exist. Verify the ARN and try again." — i.e. it builds an ARN from the bad identifier and reports the lookup miss.
Why: a client that distinguishes "you sent me garbage" (do not retry, fix the input) from "that cluster is gone" (retry elsewhere / treat as deleted) takes the wrong branch, and SDK error-type dispatch selects the wrong exception class entirely.
Fix: GetCluster's input validation, which must run the ClusterId format check and emit the modeled ValidationException before the handler consults cluster state.
Traces: dsql-cluster-error-paths#task#1.

Cause 2 of 6: DeleteCluster with the same syntactically invalid cluster identifier.
AWS: 400 ValidationException, x-amzn-errortype: ValidationException, same fixed message template as Cause 1.
Emulator: 404 ResourceNotFoundException with the ARN-not-found sentence.
Why: same as Cause 1; additionally a delete-until-gone loop reads the 404 as "already deleted" and reports success for a request AWS never accepted.
Fix: DeleteCluster's input validation, same pre-lookup identifier check.
Traces: dsql-cluster-error-paths#task#2.

Cause 3 of 6: GetClusterPolicy with a syntactically invalid cluster identifier.
AWS: 400 ValidationException, same template.
Emulator: 404 ResourceNotFoundException with the ARN-not-found sentence (x-amzn-errortype: ResourceNotFoundException).
Why: same as Cause 1.
Fix: GetClusterPolicy's input validation.
Traces: dsql-request-validation-error-surface#task#5.

Cause 4 of 6: GetStream with a streamIdentifier that is not a valid base32 stream id.
AWS: 400 ValidationException. Smithy: StreamId pattern ^[a-z0-9]{26}$; the service applies the stricter base32 rule and reports the fixed message "Aurora DSQL can't process the request because the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again." — the whole sentence pair is fixed text, with the member name as its only variable part.
Emulator: 404 ResourceNotFoundException whose message names a composite cluster/stream ARN built from the rejected value.
Why: same as Cause 1, on the CDC-stream read path.
Fix: GetStream's input validation, before the stream lookup.
Traces: dsql-cdc-stream-error-handling#task#5.

Cause 5 of 6: DeleteStream with the same invalid streamIdentifier.
AWS: 400 ValidationException with the streamIdentifier message of Cause 4.
Emulator: 404 ResourceNotFoundException with the composite-ARN sentence.
Why: same as Cause 4; a tear-down loop treats the 404 as "stream already deleted".
Fix: DeleteStream's input validation.
Traces: dsql-cdc-stream-error-handling#task#6.

Cause 6 of 6: DeleteClusterPolicy in a policy-version-guard sequence where the policy the request targets is not there to delete.
AWS: 404, the ResourceNotFoundException class (Smithy: ResourceNotFoundException httpError 404, required message/resourceId/resourceType) — the absence of the policy is decided first.
Emulator: 409 with x-amzn-errortype: ConflictException.
Why: a client branching on 404 (nothing to delete, proceed) retries or aborts on the 409 instead, and a conditional delete loop never terminates.
Hypothesis: the handler evaluates its expectedPolicyVersion guard before checking that a policy exists, so a mismatch against an absent policy surfaces as a conflict.
Fix: DeleteClusterPolicy's handler, which must order the policy-existence check ahead of the expectedPolicyVersion guard.
Traces: dsql-cluster-policy-version-guards#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (12 fixtures, 96 steps, 29 mismatched):
- dsql-cluster-error-paths#task#1 GetCluster: expected 400 / ValidationException / "Invalid input 'identifier: zzzzzzzzzzzzzzzzzzzzzzzzzz'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again."; got 404 / ResourceNotFoundException / "The resource with ARN arn:aws:dsql:us-west-2:000000000000:cluster/zzzzzzzzzzzzzzzzzzzzzzzzzz doesn't exist. Verify the ARN and try again."
- dsql-cluster-error-paths#task#2 DeleteCluster: identical expected/actual pair.
- dsql-request-validation-error-surface#task#5 GetClusterPolicy: expected 400 ValidationException with the same template (identifier bbbbbbbbbbbbbbbbbbbbbbbbbb); got 404 ResourceNotFoundException.
- dsql-cdc-stream-error-handling#task#5 GetStream: expected 400 with "...the streamIdentifier that you specified isn't valid. Provide the streamIdentifier in the format 'a valid base32-encoded 26-character string' and try again."; got 404 ResourceNotFoundException "The resource with ARN arn:aws:dsql:...:cluster/<cluster>/stream/<stream> doesn't exist. Verify the ARN and try again."
- dsql-cdc-stream-error-handling#task#6 DeleteStream: identical expected/actual pair.
- dsql-cluster-policy-version-guards#task#6 DeleteClusterPolicy: expected status 404; got 409 with x-amzn-errortype ConflictException.

## Steps to Reproduce
1. Replay fixture dsql-cluster-error-paths, step task#1: GET /cluster/<26-char identifier that is not a valid DSQL id> -> emulator answers 404 ResourceNotFoundException, AWS answers 400 ValidationException.
2. Same fixture step task#2: DELETE /cluster/<same identifier>.
3. Replay dsql-request-validation-error-surface step task#5: GET /cluster/<invalid identifier>/policy.
4. Replay dsql-cdc-stream-error-handling steps task#5 and task#6: GET and DELETE /stream/<valid cluster>/<non-base32 26-char stream id> -> 404 instead of 400.
5. Replay dsql-cluster-policy-version-guards step task#6: DeleteClusterPolicy in the version-guard sequence -> 409 ConflictException instead of 404 ResourceNotFoundException.
