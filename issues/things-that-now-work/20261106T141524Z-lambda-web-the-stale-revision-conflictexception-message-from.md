Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-web.put-resource-policy-revision-chain-rejects-stale-revision-at-each-generation
Issue-Shape-Id: com.amazonaws.lambdaweb#LambdaWeb
Issue-Key: lambda-web.put-resource-policy-revision-chain-rejects-stale-revision-at-each-generation|run#7|AssertionError|assert message == (
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# lambda-web: the stale-revision ConflictException message from PutResourcePolicy/DeleteResourcePolicy is not AWS's wording

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
One net-new root cause in lambda-web's resource-policy optimistic-concurrency path.

Cause 1 of 1: the ConflictException raised when a PutResourcePolicy (and, from the same shared constant, DeleteResourcePolicy) carries a `revisionId` that is not the stored policy's current generation reports a different message text than AWS. Failing check: lambda-web.put-resource-policy-revision-chain-rejects-stale-revision-at-each-generation.

AWS: PutResourcePolicy models ConflictException at HTTP 409 (Smithy: PutResourcePolicy errors -> ConflictException, errorKind client, httpError 409; `PutResourcePolicyRequest$revisionId` doc: "The revision ID of the existing policy. Use this to prevent conflicts when updating a policy concurrently. If you don't specify a value, the update proceeds without checking the current revision."; ConflictException doc: "...or the policy revision ID that you specified doesn't match the current policy. Resolve the conflict and try again."). For this conflict class the recorded behaviour is the fixed sentence pair "The policy was updated or deleted by another request. Try again with the latest policy revisionId." — it is entirely fixed text, carrying NO values from the request or state (no revision id, no ARN, no function name), so there is nothing in it to template per request. The HTTP status (409) and the error code (ConflictException) the emulator already returns are correct; only the message diverges. The rule holds for any stale generation, not just the immediately previous one: the check puts rev1, updates to rev2, then re-puts with rev1 (two generations stale) and must get this same refusal.

Emulator: answers ConflictException 409 with the message "The policy was updated by another request. Try again." — a different sentence (it omits "or deleted" and the "Try again with the latest policy revisionId." instruction). The replay fails with `AssertionError: unexpected conflict message 'The policy was updated by another request. Try again.'`. Everything else in that check (the 409/ConflictException classification, the refusal leaving policy and revision id untouched, and the fresh ids minted by each accepted put) behaves as recorded.

Why: a client that branches on the error message — matching the documented stale-revision sentence to decide whether to re-read the policy and retry with the latest revisionId, versus surfacing an unexpected conflict — takes the wrong branch against the emulator, and golden-output/assertion tests that pin AWS's message text fail even though the retry logic is correct. The message is part of the error contract, so a diverging sentence is a defect even with the right status and code.

Fix: the shared stale-revision message constant used by this operation's handler (the resource-policy state slice that both mutating resource-policy handlers read), so PutResourcePolicy and DeleteResourcePolicy report the same AWS wording for the one conflict class.

Traces: lambda-web.put-resource-policy-revision-chain-rejects-stale-revision-at-each-generation. Citations: services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/resource_policies.rs:61-62 defines STALE_POLICY_REVISION_MESSAGE; services/service-com-amazonaws-lambdaweb-lambdaweb/src/ops/put_resource_policy.rs:212-231 raises ConflictException with it.

## Evidence
Check lambda-web.put-resource-policy-revision-chain-rejects-stale-revision-at-each-generation, [mismatch]:

AssertionError: unexpected conflict message 'The policy was updated by another request. Try again.'
  at check_lambda_web_put_resource_policy_revision_chain_rejects_stale_revision_at_each_generation.py:144

Sequence: rev1 = PutResourcePolicy(resourceArn, policy v1); rev2 = PutResourcePolicy(resourceArn, policy v2, revisionId=rev1); then PutResourcePolicy(resourceArn, policy v3-rejected, revisionId=rev1).

Expected (AWS):   HTTP 409, Error.Code = "ConflictException", Error.Message = "The policy was updated or deleted by another request. Try again with the latest policy revisionId."  (all fixed text; no request/state values)
Actual (emulator): HTTP 409, Error.Code = "ConflictException", Error.Message = "The policy was updated by another request. Try again."

The status and code match; only the message text diverges. The same constant backs DeleteResourcePolicy's stale-revision refusal (services/service-com-amazonaws-lambdaweb-lambdaweb/src/state/resource_policies.rs:51-62).

## Steps to Reproduce
1. Create an execution role, a versioned S3 code bucket, and one Active lambda-web web function (CreateWebFunction with revisionConfig/endpointConfig as in the check).
2. rev1 = lambda-web PutResourcePolicy(resourceArn=<functionArn>, policy=<any valid JSON policy object>)["revisionId"].
3. rev2 = lambda-web PutResourcePolicy(resourceArn=<functionArn>, policy=<another policy>, revisionId=rev1)["revisionId"]  (succeeds, rev2 != rev1).
4. lambda-web PutResourcePolicy(resourceArn=<functionArn>, policy=<a third policy>, revisionId=rev1) — now two generations stale.
5. Observe the ClientError: Code is ConflictException (correct) but Message is "The policy was updated by another request. Try again." instead of AWS's "The policy was updated or deleted by another request. Try again with the latest policy revisionId."
6. The same divergence is reachable through DeleteResourcePolicy with a stale revisionId, which shares the message constant.
