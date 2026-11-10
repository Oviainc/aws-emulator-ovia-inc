Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-missing-cluster-error-surface
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-missing-cluster-error-surface
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|PutClusterPolicy|4xx->4xx|$.message
Issue-Related: 20261103T194736Z-dsql-rejects-syntactically-valid-26-character-identifiers-as.md
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# dsql PutClusterPolicy absent-cluster refusal carries wrong message, status and error shape

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutClusterPolicy

## Description
There is 1 cause to close.

Cause 1 of 1: PutClusterPolicy against a well-formed cluster identifier that names no stored cluster (fixture dsql-missing-cluster-error-surface, step task#3).

AWS: PutClusterPolicy binds `identifier` as an @httpLabel on POST /cluster/{identifier}/policy, targeting ClusterId with @pattern ^[a-z0-9]{26}$ (Smithy: PutClusterPolicy.identifier -> ClusterId, required, pattern ^[a-z0-9]{26}$). Any 26-character lowercase-alphanumeric string is therefore a WELL-FORMED identifier and must pass input validation. The operation models both ResourceNotFoundException (httpError 404) and ValidationException (httpError 400), so absence of the addressed cluster is reported as the former, not the latter. ResourceNotFoundException has three REQUIRED members (Smithy: ResourceNotFoundException.message, .resourceId 'The resource ID could not be found.', .resourceType 'The resource type could not be found.'), and the refusal is serialized with x-amzn-errortype naming ResourceNotFoundException. The recorded response gives the message for this error class as the fixed sentence "The resource with ARN " + <arn> + " doesn't exist. Verify the ARN and try again." — the fixed text is "The resource with ARN " and " doesn't exist. Verify the ARN and try again."; the only variable part is <arn>, the cluster ARN assembled from the partition, region and account of the request plus the `identifier` taken from the request path (arn:<partition>:dsql:<region>:<account>:cluster/<identifier>). resourceId is the `identifier` from the request verbatim, and resourceType is the fixed string "cluster".

Emulator: the emulator answers HTTP 400 with x-amzn-errortype: ValidationException and message "Invalid input 'identifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again.", and emits NEITHER resourceId NOR resourceType. It rejects the identifier as malformed even though the value satisfies the modeled pattern, so the store lookup that would raise the modeled absence error is never reached. Hypothesis: the identifier guard implements a stricter decode than the modeled pattern (it speaks of 'format is invalid' rather than a pattern violation) and runs ahead of any state lookup.

Why: a client cannot distinguish 'this cluster does not exist' from 'you sent a bad request'. 400 ValidationException is a non-retryable caller error, so policy-reconciliation code, create-or-attach flows and retry wrappers conclude the identifier they hold is malformed and either abort or 'correct' an identifier that was never wrong, instead of taking the absent-cluster branch. The two required members resourceId and resourceType are also absent, so callers that key off the structured fields rather than the prose get nothing at all, and SDK error-shape deserialization sees a shape it was not told to expect for this condition.

Fix: PutClusterPolicy's input validation must enforce only the modeled ClusterId pattern and stop rejecting values that satisfy it; the handler's cluster lookup must then raise ResourceNotFoundException with the 404 status, the ResourceNotFoundException errortype, the message template above, resourceId = the request's identifier and resourceType = "cluster". The serializer must populate all three required members of that error shape.

Traces: fixture dsql-missing-cluster-error-surface, step task#3.

Related open issue that does NOT cover this: 20261103T194736Z-dsql-rejects-syntactically-valid-26-character-identifiers-as.md describes the same class of fault across seven operations and names PutClusterPolicy among them, but its recorded replay keys for PutClusterPolicy cover only status, $headers.x-amzn-errortype, $.resourceId and $.resourceType — the $.message divergence on this step is NOT among them, so this step is reported unowned by replay and the wrong message text would be closed unverified by that issue alone. Fixing that issue's status/shape causes without correcting this operation's message wording leaves this divergence live.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200; 12 fixtures, 96 steps, 74 passed, 22 mismatched, score 77.1%, 17 divergence clusters). One cluster, service=dsql, carried an EMPTY owned_by and an empty stub_path:

PutClusterPolicy — fixture dsql-missing-cluster-error-surface, step task#3, verdict mismatch, divergence_key "service=dsql|PutClusterPolicy|status=404->400|keys=$.message,$.resourceId,$.resourceType,$headers.x-amzn-errortype,status", affected_count 1:
  - status: expected 404, got 400
  - $headers.x-amzn-errortype: shape_mismatch (expected the shaped ResourceNotFoundException errortype, got 'ValidationException')
  - $.resourceId: missing (expected the 26-character cluster identifier taken from the request, got None)
  - $.resourceType: missing (expected 'cluster', got None)
  - $.message: value_mismatch — expected "The resource with ARN <cluster-arn> doesn't exist. Verify the ARN and try again." where <cluster-arn> is the cluster ARN built from the request's partition/region/account plus the request's identifier; got "Invalid input 'identifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again."

Model grounding (services/service-com-amazonaws-dsql-dsql/smithy-model/com-amazonaws-dsql-dsql.json):
  - PutClusterPolicy http: POST /cluster/{identifier}/policy, success code 200, idempotent.
  - PutClusterPolicy.identifier -> ClusterId, required, @httpLabel, @pattern ^[a-z0-9]{26}$ — the replayed identifier satisfies this pattern.
  - PutClusterPolicy errors include ResourceNotFoundException (client, httpError 404) AND ValidationException (client, httpError 400), so the modeled absence path exists and is distinct from the input-validation path.
  - ResourceNotFoundException members: message (required), resourceId (required), resourceType (required) — the emulator emits only a message, and that message is the ValidationException wording.
  - ValidationException members: message (required), reason (required enum), fieldList — the emulator's 400 is being used for a condition the model classes as 404.

## Steps to Reproduce
1. git clone the repository and boot the emulator from that checkout; confirm /__admin/health answers 200 and the data listener accepts connections.
2. Replay wire fixture dsql-missing-cluster-error-surface against an empty dsql store and watch step task#3.
3. That step issues PutClusterPolicy as POST /cluster/{identifier}/policy with a 26-character lowercase-alphanumeric cluster identifier that satisfies ClusterId's @pattern ^[a-z0-9]{26}$ and that no stored cluster uses, plus a valid policy document.
4. Expected (AWS): HTTP 404 with header x-amzn-errortype naming ResourceNotFoundException and a body carrying all three required members — message "The resource with ARN <arn> doesn't exist. Verify the ARN and try again." with <arn> = arn:<partition>:dsql:<region>:<account>:cluster/<identifier> assembled from the request, resourceId = the identifier from the request path, resourceType = "cluster".
5. Observed (emulator): HTTP 400 with header x-amzn-errortype: ValidationException and a body whose message is "Invalid input 'identifier: <id>'. The cluster identifier format is invalid. Provide a valid cluster identifier and try again.", with no resourceId and no resourceType members at all.
6. Re-replay the same fixture/step after the fix and confirm the status, the errortype header, the message wording and both structured members.
