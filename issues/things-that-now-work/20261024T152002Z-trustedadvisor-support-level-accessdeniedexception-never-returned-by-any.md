Issue-Source: trace-sweep-divergence
Issue-Trace: trustedadvisor-account-level-mutation-access-denied
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: trustedadvisor-account-level-mutation-access-denied trustedadvisor-account-level-read-access-denied
Issue-Shape-Id: com.amazonaws.trustedadvisor#TrustedAdvisor
Issue-Key: trustedadvisor|GetRecommendation|4xx->4xx|$.message
Issue-Key: trustedadvisor|GetRecommendation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: trustedadvisor|GetRecommendation|4xx->4xx|status
Issue-Key: trustedadvisor|ListRecommendationResources|4xx->4xx|$.message
Issue-Key: trustedadvisor|ListRecommendationResources|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: trustedadvisor|ListRecommendationResources|4xx->4xx|status
Issue-Key: trustedadvisor|UpdateRecommendationLifecycle|4xx->4xx|$.message
Issue-Key: trustedadvisor|UpdateRecommendationLifecycle|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: trustedadvisor|UpdateRecommendationLifecycle|4xx->4xx|status
Issue-Key: trustedadvisor|BatchUpdateRecommendationResourceExclusion|4xx->2xx|$.message
Issue-Key: trustedadvisor|BatchUpdateRecommendationResourceExclusion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: trustedadvisor|BatchUpdateRecommendationResourceExclusion|4xx->2xx|status
Issue-Key: trustedadvisor|ListChecks|4xx->2xx|$.message
Issue-Key: trustedadvisor|ListChecks|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: trustedadvisor|ListChecks|4xx->2xx|status
Issue-Key: trustedadvisor|ListRecommendations|4xx->2xx|$.message
Issue-Key: trustedadvisor|ListRecommendations|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: trustedadvisor|ListRecommendations|4xx->2xx|status
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# trustedadvisor support-level AccessDeniedException never returned by any operation

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendation

## Description
The emulator never models the Trusted Advisor account entitlement (support-level) gate. Every operation in the trustedadvisor Recommendation/Check API is modelled with AccessDeniedException @httpError 403 (Smithy: AccessDeniedException, errorKind client, httpError 403; documentation "Exception that access has been denied due to insufficient access"), and the recorded corpus shows an account whose support plan does not entitle it to the API: every call is answered 403 with x-amzn-errortype AccessDeniedException. The emulator instead evaluates each request on its merits — it either resolves the request against empty state and reports a 404 ResourceNotFoundException, or succeeds outright with 200 — so the entitlement check is absent from both the read and the mutation paths. This is ONE defect with SIX causes, one per operation, listed below; all six must be closed.

Cause 1 of 6: GetRecommendation (fixture trustedadvisor-account-level-mutation-access-denied, step task#1).
AWS: Smithy GetRecommendation GET /v1/recommendations/{recommendationIdentifier}, errors include AccessDeniedException (client, httpError 403) and ResourceNotFoundException (client, httpError 404). For an account that is not entitled to the API the recorded response is HTTP 403 with header x-amzn-errortype: AccessDeniedException; the entitlement decision precedes any lookup of the recommendation identifier. Recorded message, as an example only: "Access denied due to support level".
Emulator: returns HTTP 404 with x-amzn-errortype ResourceNotFoundException and a body message reporting that no recommendation exists with the requested identifier.
Why: a client cannot distinguish "your support plan does not include Trusted Advisor" from "that recommendation does not exist". SDK error handling branches on the error code, so callers that are expected to surface an entitlement/upgrade path instead treat the account as entitled-but-empty and retry or silently show no findings.
Fix: the entitlement/authorization gate in front of the GetRecommendation handler in the trustedadvisor service crate's operation dispatch, evaluated before identifier resolution.
Traces: trustedadvisor-account-level-mutation-access-denied#task#1.

Cause 2 of 6: ListRecommendationResources (fixture trustedadvisor-account-level-mutation-access-denied, step task#2).
AWS: Smithy ListRecommendationResources GET /v1/recommendations/{recommendationIdentifier}/resources, errors include AccessDeniedException (403) and ResourceNotFoundException (404). Unentitled account: HTTP 403, x-amzn-errortype AccessDeniedException, before the recommendation identifier is resolved.
Emulator: returns HTTP 404 with x-amzn-errortype ResourceNotFoundException and a no-such-recommendation message.
Why: same as Cause 1 — the paginating client sees an empty/missing resource rather than a denial, so it reports no affected resources instead of an access failure.
Fix: the same pre-dispatch entitlement gate, applied to the ListRecommendationResources handler.
Traces: trustedadvisor-account-level-mutation-access-denied#task#2.

Cause 3 of 6: UpdateRecommendationLifecycle (fixture trustedadvisor-account-level-mutation-access-denied, step task#3).
AWS: Smithy UpdateRecommendationLifecycle PUT /v1/recommendations/{recommendationIdentifier}/lifecycle, errors include AccessDeniedException (403), ResourceNotFoundException (404), ConflictException (409), ValidationException (400). Unentitled account: HTTP 403, x-amzn-errortype AccessDeniedException.
Emulator: returns HTTP 404 with x-amzn-errortype ResourceNotFoundException.
Why: a mutation that AWS refuses on authorization grounds is reported as a missing target, so a client retries against other identifiers or concludes its state is stale rather than that it is not permitted to write at all.
Fix: the entitlement gate must cover mutating handlers too, in the UpdateRecommendationLifecycle handler's authorization step.
Traces: trustedadvisor-account-level-mutation-access-denied#task#3.

Cause 4 of 6: BatchUpdateRecommendationResourceExclusion (fixture trustedadvisor-account-level-mutation-access-denied, step task#4).
AWS: Smithy BatchUpdateRecommendationResourceExclusion PUT /v1/batch-update-recommendation-resource-exclusion, errors include AccessDeniedException (403), ConflictException (409), ValidationException (400) — note there is no ResourceNotFoundException on this operation. Unentitled account: HTTP 403, x-amzn-errortype AccessDeniedException.
Emulator: returns HTTP 200 with a success response body and no error type header — the mutation is ACCEPTED.
Why: this is an authorization bypass, not just a wrong code. A client that AWS forbids from excluding resources is told the exclusion succeeded, so its local view of exclusions diverges permanently from the service's, and any workflow gated on the denial (prompting for a support-plan upgrade, falling back to read-only) is never taken.
Fix: the entitlement gate in the BatchUpdateRecommendationResourceExclusion handler, before any exclusion is written to state.
Traces: trustedadvisor-account-level-mutation-access-denied#task#4.

Cause 5 of 6: ListChecks (fixture trustedadvisor-account-level-read-access-denied, step task#1).
AWS: Smithy ListChecks GET /v1/checks, errors include AccessDeniedException (403), ThrottlingException (429), ValidationException (400), InternalServerException (500). Unentitled account: HTTP 403, x-amzn-errortype AccessDeniedException.
Emulator: returns HTTP 200 with no error type header — the listing succeeds.
Why: the check catalogue is served to an account AWS denies, so a client concludes it is entitled to the Trusted Advisor API and proceeds to call the dependent recommendation operations, which on AWS would all be refused.
Fix: the entitlement gate applied to the read handlers as well, in the ListChecks handler's authorization step.
Traces: trustedadvisor-account-level-read-access-denied#task#1.

Cause 6 of 6: ListRecommendations (fixture trustedadvisor-account-level-read-access-denied, step task#2).
AWS: Smithy ListRecommendations GET /v1/recommendations, errors include AccessDeniedException (403), ThrottlingException (429), ValidationException (400), InternalServerException (500). Unentitled account: HTTP 403, x-amzn-errortype AccessDeniedException.
Emulator: returns HTTP 200 with no error type header — the listing succeeds.
Why: an unentitled account is shown an (empty) recommendation list instead of a denial, which a client reads as "nothing to remediate" — the most damaging possible misreading of a Trusted Advisor response.
Fix: the entitlement gate applied to the ListRecommendations handler.
Traces: trustedadvisor-account-level-read-access-denied#task#2.

Common fix: both recorded fixtures exercise the same account entitlement state, so the right place for this is one shared support-level/entitlement check in the trustedadvisor service crate's request dispatch (services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/service.rs and src/ops/), evaluated for every bound operation before input resolution, emitting AccessDeniedException serialized as HTTP 403 with x-amzn-errortype: AccessDeniedException. The two open trustedadvisor issues (20261023T085619Z-trustedadvisor-getorganizationrecommendation-stub.md and 20261023T085619Z-trustedadvisor-listorganizationrecommendationresources-stub.md) cover unimplemented Organization-scoped operations and do NOT cover any of the six causes above.

## Evidence
Replay of trace set simparity-wire-ga-prod-v1: 7 steps, 1 passed, 6 mismatched, score 14.3%. All six mismatches are trustedadvisor and all six expect HTTP 403 / x-amzn-errortype AccessDeniedException.

1. GetRecommendation - trustedadvisor-account-level-mutation-access-denied#task#1: expected status 403, got 404; $headers.x-amzn-errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'; $.message expected 'Access denied due to support level' (example), got a no-such-recommendation message.
2. ListRecommendationResources - trustedadvisor-account-level-mutation-access-denied#task#2: expected 403, got 404; errortype AccessDeniedException -> ResourceNotFoundException.
3. UpdateRecommendationLifecycle - trustedadvisor-account-level-mutation-access-denied#task#3: expected 403, got 404; errortype AccessDeniedException -> ResourceNotFoundException.
4. BatchUpdateRecommendationResourceExclusion - trustedadvisor-account-level-mutation-access-denied#task#4: expected 403, got 200; $headers.x-amzn-errortype missing (expected 'AccessDeniedException'); $.message missing. Mutation accepted where AWS denies.
5. ListChecks - trustedadvisor-account-level-read-access-denied#task#1: expected 403, got 200; errortype header missing.
6. ListRecommendations - trustedadvisor-account-level-read-access-denied#task#2: expected 403, got 200; errortype header missing.

Model support (services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json): AccessDeniedException is bound as a client error with httpError 403 on all six operations; BatchUpdateRecommendationResourceExclusion does not even model ResourceNotFoundException, so 403 is the only denial code available to it. No divergence carried a stub_path or owned_by marker.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-ga-prod-v1 (replay_wire_corpus with no arguments).
3. Fixture trustedadvisor-account-level-mutation-access-denied, steps task#1..task#4: call GetRecommendation (GET /v1/recommendations/{arn}), ListRecommendationResources (GET /v1/recommendations/{arn}/resources), UpdateRecommendationLifecycle (PUT /v1/recommendations/{arn}/lifecycle) and BatchUpdateRecommendationResourceExclusion (PUT /v1/batch-update-recommendation-resource-exclusion) as the recorded unentitled account. Each must answer HTTP 403 with x-amzn-errortype: AccessDeniedException; steps 1-3 answer 404 ResourceNotFoundException and step 4 answers 200 success.
4. Fixture trustedadvisor-account-level-read-access-denied, steps task#1..task#2: call ListChecks (GET /v1/checks) and ListRecommendations (GET /v1/recommendations) as the same account. Each must answer HTTP 403 with x-amzn-errortype: AccessDeniedException; both answer 200.
5. After the fix, re-replay both fixtures and confirm all six steps return 403 with the AccessDeniedException error type.
