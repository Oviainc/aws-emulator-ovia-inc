Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.origin-request-policy-lifecycle
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.origin-request-policy-lifecycle|run#15|AssertionError|assert update_code == "NoSuchOriginRequestPolicy", (
Issue-Commit: 4c4d1dd9bb8e208540618098018eeada85664900

# cloudfront: UpdateOriginRequestPolicy answers 501 NotImplemented, so a deleted policy id is still writable instead of refused 404 NoSuchOriginRequestPolicy

> _Note: this issue was identified against the code base at commit `4c4d1dd9bb8e208540618098018eeada85664900`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOriginRequestPolicy

## Description
This issue carries ONE net-new root cause. A fixer must close it before submitting.

Related open issue: 20261024T182555Z-cloudfront-six-control-plane-operations-answer-501-notimplemented.md describes the same stub (its Cause 3 of 6, UpdateOriginRequestPolicy) but is grounded only in the quantity-consistency and concurrent-update facets (cloudfront.update-origin-request-policy-inconsistent-quantities-fails, -rejected, cloudfront.concurrent-update-origin-request-policy-single-winner). It does NOT carry the replay key for cloudfront.origin-request-policy-lifecycle — the replay report marks that check neither ALREADY FILED nor OWNED — so the not-found gate described below is unowned and would otherwise be closed unrun. Close the two together if convenient, but this check id must be replayed to confirm the fix.

Cause 1 of 1: UpdateOriginRequestPolicy is a scaffold and refuses every call, so the not-found gate on a deleted origin-request-policy id is unobservable.

AWS: Smithy: UpdateOriginRequestPolicy, PUT /2020-05-31/origin-request-policy/{Id}, success 200, Id @httpLabel required, IfMatch bound to the If-Match header, OriginRequestPolicyConfig the @httpPayload. Its modeled errors include NoSuchOriginRequestPolicy (HTTP 404, doc "The origin request policy does not exist."), InvalidIfMatchVersion (400), PreconditionFailed (412), InconsistentQuantities (400), IllegalUpdate (400). When the Id names a policy that no longer exists, AWS resolves the id BEFORE it evaluates the If-Match precondition and refuses the mutation 404 NoSuchOriginRequestPolicy. Recorded behavior for the message: fixed text "The specified origin request policy does not exist." — the whole string is fixed text for this error class, with no variable part interpolated from the request (the policy id does not appear in it). The same error class is already produced correctly by GetOriginRequestPolicy on the same dead id in this very check, so the string is available in the emulator today; only the mutation path is missing it. A deleted id must be unusable, not merely unreadable.

Emulator: every UpdateOriginRequestPolicy call answers HTTP 501 with Code=NotImplemented and Message="operation UpdateOriginRequestPolicy is scaffolded but not implemented in this emulator", for valid and invalid input alike. services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_origin_request_policy.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateOriginRequestPolicy"))`, so the handler never reads state and no id resolution, If-Match check or config validation runs. The modeled error enum UpdateOriginRequestPolicyErrorModeled in that file already declares NoSuchOriginRequestPolicy, so nothing in the model blocks the correct refusal.

Why: a client gets a 5xx where AWS gives it a 4xx it must branch on. botocore classifies 501 NotImplemented as a retryable server fault, so a caller that updates a policy another actor has deleted retries the doomed call to its retry ceiling instead of taking the NoSuchOriginRequestPolicy branch and recreating the policy; a caller that probes an id's liveness by attempting a write cannot distinguish "this id is gone" from "the service is broken". More broadly an origin request policy can be created and read but never changed, so no header/cookie/query-string allow-list can be edited after creation.

Fix: the UpdateOriginRequestPolicy handler (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_origin_request_policy.rs) and the origin-request-policy slice of crate state it must read: resolve Id against stored policies first and return NoSuchOriginRequestPolicy when absent, then evaluate If-Match (InvalidIfMatchVersion / PreconditionFailed), then validate the config (InconsistentQuantities) before writing and re-issuing the ETag. Delete the SCAFFOLDED marker line once implemented.

Traces: cloudfront.origin-request-policy-lifecycle.

## Evidence
Replay output, verbatim tail (cloudfront.origin-request-policy-lifecycle):

  File "check_cloudfront_origin_request_policy_lifecycle.py", line 71, in run
    assert update_code == "NoSuchOriginRequestPolicy", (
  AssertionError: expected NoSuchOriginRequestPolicy updating a deleted policy, got NotImplemented

The check reached that assertion, so the preceding steps all behaved: CreateOriginRequestPolicy succeeded, GetOriginRequestPolicy round-tripped HeadersConfig.Headers.Items == ["CloudFront-Viewer-Country"], DeleteOriginRequestPolicy with the live ETag succeeded, and the post-delete GetOriginRequestPolicy was correctly refused Code=NoSuchOriginRequestPolicy with Message="The specified origin request policy does not exist.". Only the mutation on the dead id diverges.

Expected (AWS): HTTP 404, Error.Code=NoSuchOriginRequestPolicy, Error.Message="The specified origin request policy does not exist." (entirely fixed text for this error class; no request- or state-derived substitution).
Actual (emulator): HTTP 501, Error.Code=NotImplemented, Error.Message="operation UpdateOriginRequestPolicy is scaffolded but not implemented in this emulator".

Source confirming the cause: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/update_origin_request_policy.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 193 (`Err(crate::scaffolded::Scaffolded::not_implemented("UpdateOriginRequestPolicy"))`); line 122 shows NoSuchOriginRequestPolicy is already an available modeled variant.

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudfront client at it.
2. CreateOriginRequestPolicy(OriginRequestPolicyConfig={Name=<unique>, Comment=..., HeadersConfig={HeaderBehavior:"whitelist", Headers:{Quantity:1, Items:["CloudFront-Viewer-Country"]}}, CookiesConfig={CookieBehavior:"none"}, QueryStringsConfig={QueryStringBehavior:"none"}}) and keep the returned Id.
3. GetOriginRequestPolicy(Id=<id>) and keep its ETag.
4. DeleteOriginRequestPolicy(Id=<id>, IfMatch=<that ETag>) -> succeeds.
5. GetOriginRequestPolicy(Id=<id>) -> correctly refused 404 NoSuchOriginRequestPolicy / "The specified origin request policy does not exist.".
6. UpdateOriginRequestPolicy(Id=<id>, IfMatch=<that ETag>, OriginRequestPolicyConfig=<any valid config>) -> observe HTTP 501 Code=NotImplemented instead of the expected HTTP 404 Code=NoSuchOriginRequestPolicy with the same message as step 5.
Or replay the check id in trace_ids: cloudfront.origin-request-policy-lifecycle.
