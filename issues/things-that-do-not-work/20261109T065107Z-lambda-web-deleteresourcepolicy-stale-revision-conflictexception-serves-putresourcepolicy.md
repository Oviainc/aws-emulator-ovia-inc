Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-web-resource-policy-and-tag-lifecycle
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-web-resource-policy-and-tag-lifecycle
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Related: 20261107T125554Z-lambda-web-resource-policy-and-tag-refusals-diverge.md
Issue-Commit: 1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2

# lambda-web DeleteResourcePolicy stale-revision ConflictException serves PutResourcePolicy's wording

> _Note: this issue was identified against the code base at commit `1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
ONE net-new root cause in the lambda-web emulator's resource-policy error contract, reached by the replayed wire corpus.

--------------------------------------------------------------------
Cause 1 of 1: DeleteResourcePolicy's stale-revision ConflictException reports the sentence AWS uses for PutResourcePolicy instead of the shorter sentence AWS uses for the delete. The emulator serves ONE shared message constant to both mutating resource-policy operations, but AWS words the two refusals differently, so whichever single sentence the constant holds, one of the two operations diverges.
Fixture/step: lambda-web-resource-policy-and-tag-lifecycle#task#11

AWS: Smithy (services/service-com-amazonaws-lambdaweb-lambdaweb/smithy-model/com-amazonaws-lambdaweb-lambdaweb.json), service LambdaWeb, restJson1. smithy_query(query="operation", operation="DeleteResourcePolicy") gives DELETE /2025-03-07/resource-policy/{resourceArn}, success 204, @idempotent; input resourceArn (ResourceArn, @required, @httpLabel, @length 1..256, @pattern ^arn:[^:]+:lambda:[^:]+:\d{12}:web-function/[^/]+$) and revisionId (PolicyRevisionId, @httpQuery "RevisionId", @pattern ^[0-9a-f-]{36}$); modelled errors include ConflictException (client, httpError 409). smithy_query(query="doc", shape="ConflictException") documents the class as "The request conflicts with the current state of the resource. For example, the resource is being modified by another request, you tried to delete a revision that is serving traffic on an endpoint, or the policy revision ID that you specified doesn't match the current policy. Resolve the conflict and try again." The model does NOT fix the message text and does not state that the two operations share one, so the per-operation wording is recorded behavior only. For a DeleteResourcePolicy carrying a RevisionId that is not the stored policy's current generation, AWS answers HTTP 409, error code ConflictException, message `The policy was updated by another request. Try again.` That message is ENTIRELY FIXED TEXT: it has NO variable parts — no revision id, no resource ARN, no function name, no count. The refusal is total: the stored policy and its revision survive unchanged. Note that this is NOT the sentence AWS returns for the same conflict class on PutResourcePolicy (which in the same corpus run expects `The policy was updated or deleted by another request. Try again with the latest policy revisionId.`, and whose $.message now matches): the two operations have DISTINCT fixed texts for the one modelled error shape.

Emulator: status 409 and error code ConflictException are correct, but the message is `The policy was updated or deleted by another request. Try again with the latest policy revisionId.` — the PutResourcePolicy sentence, served because the delete handler reads the same single shared constant as the put. The replay reports `$.message: value_mismatch (expected 'The policy was updated by another request. Try again.', got 'The policy was updated or deleted by another request. Try again with the latest policy revisionId.')`.

Why: a client or provider that matches AWS's documented delete-conflict sentence to decide "the policy moved on, re-read it and retry" versus "surface an unexpected conflict" sees a sentence it does not recognise, and cannot tell a stale-revision delete refusal from a stale-revision put refusal because the emulator makes the two indistinguishable by text. The error message is part of the error contract even when status and code are right, so golden-output assertions and message-matching retry logic diverge on the emulator only.

Fix: the shared stale-revision message in the resource-policy slice of this service's state must stop being ONE constant for both readers — DeleteResourcePolicy's 409 branch in its handler needs its own fixed sentence, distinct from the one PutResourcePolicy's 409 branch reports. Keep the 409 status, the ConflictException classification and the leave-the-policy-untouched semantics, which are all correct.

Also in this step, and NOT part of this cause: the same 409 body carries $.resourceId and $.resourceType, which AWS omits. That is already owned by open issue 20261107T125554Z-lambda-web-resource-policy-and-tag-refusals-diverge.md (its Cause 4 explicitly lists lambda-web-resource-policy-and-tag-lifecycle#task#11), so it is not re-filed here. That issue's Cause 5 states the stale-revision wording as a SINGLE shared sentence and therefore does NOT cover this divergence: it asserts the put's wording for both operations, which is exactly what the emulator now does and what this step still refuses.

--------------------------------------------------------------------
Traces (failing fixture steps, as examples; file/line citations are against this checkout and may have drifted):
- Cause 1: lambda-web-resource-policy-and-tag-lifecycle#task#11 — services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/resource_policies.rs:64-66 (STALE_POLICY_REVISION_MESSAGE, documented at :51-63 as "The `message` both mutating resource-policy operations report"), read by services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/delete_resource_policy.rs:219-228 (the `!policy.accepts_revision(...)` branch) and by the corresponding branch of src/ops/put_resource_policy.rs.

## Evidence
Replay of trace set simparity-wire-aws-v1: 48 steps, 40 passed, 8 mismatched (score 83.3%), 7 divergence clusters, all in service lambda-web. Six clusters are reported owned by open issue 20261107T125554Z-lambda-web-resource-policy-and-tag-refusals-diverge.md; this cluster (divergence_key `service=lambda-web|DeleteResourcePolicy|status=409->409|keys=$.message,$.resourceId,$.resourceType`) is reported with an EMPTY owned_by, because that issue's Issue-Key list carries lambda-web|DeleteResourcePolicy|4xx->4xx|$.resourceId and |$.resourceType but no |$.message key for DeleteResourcePolicy. No step was blocked by a stub.

Cause 1 — lambda-web-resource-policy-and-tag-lifecycle#task#11, DeleteResourcePolicy with a stale RevisionId (a policy exists; the id presented is a previous generation's):
  Expected (AWS): HTTP 409, error code ConflictException, $.message = "The policy was updated by another request. Try again." — entirely fixed text, no request- or state-derived parts.
  Actual (emulator): HTTP 409, ConflictException (both correct), $.message = "The policy was updated or deleted by another request. Try again with the latest policy revisionId."
  Replay detail (verbatim): `$.message: value_mismatch (expected 'The policy was updated by another request. Try again.', got 'The policy was updated or deleted by another request. Try again with the latest policy revisionId.')` / `$.resourceId: type_mismatch (expected None, got 'arn:aws:lambda:us-west-2:000000000000:web-function/sim2real-probe-lambdaweb-fn-3c1ac66d')` / `$.resourceType: type_mismatch (expected None, got 'ResourcePolicy')`. The ARN shown is this run's instance of a per-environment value and is not part of the requirement; the two resourceId/resourceType mismatches are the already-owned extra-member cause, not this one.
  Cross-check in the same run: lambda-web-resource-policy-and-tag-lifecycle#task#10 (PutResourcePolicy, stale revisionId) diverges ONLY on $.resourceId/$.resourceType — its $.message now matches AWS's "The policy was updated or deleted by another request. Try again with the latest policy revisionId." So the one shared constant currently satisfies the put and breaks the delete; the two recordings prove AWS's texts differ per operation.
  Model evidence: smithy_query(query="operation", operation="DeleteResourcePolicy") — ConflictException at httpError 409, no modelled message text; smithy_query(query="doc", shape="ConflictException") — the class doc quoted in the description, which fixes neither sentence.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore `lambda-web` client at the data listener (region us-west-2, any credentials).
2. Create an execution role (iam) and a versioned code bucket (s3) holding code.zip, then CreateWebFunction with revisionConfig{buildConfig{codeConfig.s3Object, runtimeConfig.runtime=nodejs24.x}, serviceConfig.executionRoleArn} and endpointConfig{endpointName=ep, endpointType=HomeRegion, authType=IamAuth}. Keep the returned functionArn.
3. rev1 = PutResourcePolicy(resourceArn=functionArn, Policy=<a JSON policy object p1>)["RevisionId"].
4. rev2 = PutResourcePolicy(resourceArn=functionArn, Policy=<p2>, RevisionId=rev1)["RevisionId"] — the stored generation is now rev2, so rev1 is stale.
5. DeleteResourcePolicy(resourceArn=functionArn, RevisionId=rev1). The emulator correctly refuses with HTTP 409 / ConflictException and leaves the policy in place, but the body's message is "The policy was updated or deleted by another request. Try again with the latest policy revisionId." instead of AWS's "The policy was updated by another request. Try again."
6. For contrast, PutResourcePolicy(resourceArn=functionArn, Policy=<p3>, RevisionId=rev1) returns the longer sentence, which IS what AWS returns for the put — showing the single shared constant cannot serve both operations.
7. Re-replay the wire corpus to confirm: fixture lambda-web-resource-policy-and-tag-lifecycle, step task#11 (and check that task#10's $.message stays matching).
