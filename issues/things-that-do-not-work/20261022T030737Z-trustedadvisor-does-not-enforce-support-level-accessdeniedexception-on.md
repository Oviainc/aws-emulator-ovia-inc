Issue-Source: trace-sweep-divergence
Issue-Trace: trustedadvisor-account-level-mutation-access-denied
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
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
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# TrustedAdvisor does not enforce support-level AccessDeniedException on account-level recommendation APIs

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendation

## Description
The TrustedAdvisor emulator has no notion of the caller's support-plan entitlement, so the account-level Recommendation and Check APIs never return the modeled AccessDeniedException. Depending on the operation the emulator either falls through to a resource lookup (404 ResourceNotFoundException) or performs the call successfully (200), instead of refusing the request with 403 AccessDeniedException. The fault is shared by all six causes below: the authorization/entitlement gate that must run BEFORE any identifier resolution or mutation is simply absent. 6 causes.

Cause 1 of 6 — GetRecommendation (fixture trustedadvisor-account-level-mutation-access-denied, step task#1).
AWS: Smithy model GetRecommendation (GET /v1/recommendations/{recommendationIdentifier}) binds error AccessDeniedException, errorKind client, httpError 403, documented as "Exception that access has been denied due to insufficient access". When the calling account is not entitled to the TrustedAdvisor Recommendation API, AWS rejects the call with 403 and x-amzn-errortype: AccessDeniedException before resolving the recommendation ARN. Recorded response: status 403, x-amzn-errortype AccessDeniedException, message (example only) 'Access denied due to support level'.
Emulator: returns 404 with x-amzn-errortype: ResourceNotFoundException and message 'no recommendation with the identifier arn:aws:trustedadvisor::000000000000:recommendation/00000000-0000-0000-0000-000000000000'.
Why: the entitlement check is skipped and the request proceeds to identifier resolution; the client sees a retriable/permanent-mismatch 404 "not found" and concludes the recommendation does not exist, rather than learning its account lacks access. 404 vs 403 also routes to a different SDK error type, so error handling and support-plan onboarding paths are never exercised.
Fix: GetRecommendation's handler — add the support-level/entitlement gate ahead of recommendation lookup and emit AccessDeniedException (403) from the error serializer.
Traces: trustedadvisor-account-level-mutation-access-denied#task#1.

Cause 2 of 6 — ListRecommendationResources (fixture trustedadvisor-account-level-mutation-access-denied, step task#2).
AWS: Smithy model ListRecommendationResources (GET /v1/recommendations/{recommendationIdentifier}/resources) binds AccessDeniedException, client, httpError 403. Recorded response: status 403, x-amzn-errortype AccessDeniedException, message (example only) 'Access denied due to support level'.
Emulator: returns 404 with x-amzn-errortype: ResourceNotFoundException and a 'no recommendation with the identifier ...' message.
Why: same ordering defect — the unentitled caller is told the recommendation does not exist. A paginating client treats this as an empty/absent resource set instead of a permission failure.
Fix: ListRecommendationResources' handler — entitlement gate before recommendation resolution and pagination.
Traces: trustedadvisor-account-level-mutation-access-denied#task#2.

Cause 3 of 6 — UpdateRecommendationLifecycle (fixture trustedadvisor-account-level-mutation-access-denied, step task#3).
AWS: Smithy model UpdateRecommendationLifecycle (PUT /v1/recommendations/{recommendationIdentifier}/lifecycle) binds AccessDeniedException, client, httpError 403. Recorded response: status 403, x-amzn-errortype AccessDeniedException, message (example only) 'Access denied due to support level'.
Emulator: returns 404 with x-amzn-errortype: ResourceNotFoundException.
Why: a mutating call that AWS refuses on entitlement grounds is reported as a missing resource, so a client cannot distinguish "you may not do this" from "this recommendation is gone" and may retry against other identifiers.
Fix: UpdateRecommendationLifecycle's handler — entitlement gate before identifier resolution and lifecycle mutation.
Traces: trustedadvisor-account-level-mutation-access-denied#task#3.

Cause 4 of 6 — BatchUpdateRecommendationResourceExclusion (fixture trustedadvisor-account-level-mutation-access-denied, step task#4).
AWS: Smithy model BatchUpdateRecommendationResourceExclusion (PUT /v1/batch-update-recommendation-resource-exclusion) binds AccessDeniedException, client, httpError 403; note it does NOT bind ResourceNotFoundException, so 403 is the only correct refusal here. Recorded response: status 403, x-amzn-errortype AccessDeniedException, message (example only) 'Access denied due to support level'.
Emulator: returns 200 with no error payload at all (no $.message, no x-amzn-errortype) — the exclusion mutation is accepted.
Why: this is a silent authorization bypass on a state-changing operation. The emulator mutates exclusion state for a caller AWS would reject, so emulated state diverges permanently from what the real service would hold and client code that depends on being denied is never exercised.
Fix: BatchUpdateRecommendationResourceExclusion's handler — entitlement gate before applying any exclusion to state.
Traces: trustedadvisor-account-level-mutation-access-denied#task#4.

Cause 5 of 6 — ListChecks (fixture trustedadvisor-account-level-read-access-denied, step task#1).
AWS: Smithy model ListChecks (GET /v1/checks) binds AccessDeniedException, client, httpError 403. Recorded response: status 403, x-amzn-errortype AccessDeniedException, message (example only) 'Access denied due to support level'.
Emulator: returns 200 with no error payload (no $.message, no x-amzn-errortype) — a check list is served to an unentitled caller.
Why: the client believes it has TrustedAdvisor read access and proceeds to consume check summaries; against AWS it would get 403 and need to surface a support-plan requirement.
Fix: ListChecks' handler — entitlement gate before serving check summaries.
Traces: trustedadvisor-account-level-read-access-denied#task#1.

Cause 6 of 6 — ListRecommendations (fixture trustedadvisor-account-level-read-access-denied, step task#2).
AWS: Smithy model ListRecommendations (GET /v1/recommendations) binds AccessDeniedException, client, httpError 403. Recorded response: status 403, x-amzn-errortype AccessDeniedException, message (example only) 'Access denied due to support level'.
Emulator: returns 200 with no error payload (no $.message, no x-amzn-errortype).
Why: same read-path bypass — recommendation summaries are served where AWS refuses the caller, so permission-denied handling is untested and the emulator over-reports capability.
Fix: ListRecommendations' handler — entitlement gate before serving recommendation summaries.
Traces: trustedadvisor-account-level-read-access-denied#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200; state_reset_failures 0; 7 steps replayed, 0 passed, 6 divergence clusters — all trustedadvisor).

Expected (recorded AWS) for every step below: HTTP 403, header x-amzn-errortype: AccessDeniedException, body $.message 'Access denied due to support level' (message text is an example; the status and error code are the requirement).

Actual from the emulator:
1. GetRecommendation — trustedadvisor-account-level-mutation-access-denied#task#1: status 403 -> 404; $headers.x-amzn-errortype AccessDeniedException -> ResourceNotFoundException; $.message -> 'no recommendation with the identifier arn:aws:trustedadvisor::000000000000:recommendation/00000000-0000-0000-0000-000000000000'.
2. ListRecommendationResources — trustedadvisor-account-level-mutation-access-denied#task#2: status 403 -> 404; errortype AccessDeniedException -> ResourceNotFoundException; message -> 'no recommendation with the identifier ...'.
3. UpdateRecommendationLifecycle — trustedadvisor-account-level-mutation-access-denied#task#3: status 403 -> 404; errortype AccessDeniedException -> ResourceNotFoundException; message -> 'no recommendation with the identifier ...'.
4. BatchUpdateRecommendationResourceExclusion — trustedadvisor-account-level-mutation-access-denied#task#4: status 403 -> 200; $headers.x-amzn-errortype missing (expected AccessDeniedException, got None); $.message missing (expected 'Access denied due to support level', got None).
5. ListChecks — trustedadvisor-account-level-read-access-denied#task#1: status 403 -> 200; x-amzn-errortype missing; $.message missing.
6. ListRecommendations — trustedadvisor-account-level-read-access-denied#task#2: status 403 -> 200; x-amzn-errortype missing; $.message missing.

Model evidence (services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json): all six operations bind AccessDeniedException with errorKind client and httpError 403; AccessDeniedException is documented as "Exception that access has been denied due to insufficient access". BatchUpdateRecommendationResourceExclusion does not bind ResourceNotFoundException at all. No open issue covers trustedadvisor (list_existing_issues query 'trustedadvisor' returned none).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Fixture trustedadvisor-account-level-mutation-access-denied, step task#1 (GetRecommendation GET /v1/recommendations/arn:aws:trustedadvisor::000000000000:recommendation/00000000-0000-0000-0000-000000000000) as a caller without TrustedAdvisor support-level entitlement: observe 404/ResourceNotFoundException instead of 403/AccessDeniedException.
4. Same fixture, step task#2 (ListRecommendationResources on that ARN) and step task#3 (UpdateRecommendationLifecycle PUT on that ARN): observe 404/ResourceNotFoundException instead of 403/AccessDeniedException.
5. Same fixture, step task#4 (BatchUpdateRecommendationResourceExclusion PUT /v1/batch-update-recommendation-resource-exclusion): observe 200 with no error payload instead of 403/AccessDeniedException — the mutation is accepted.
6. Fixture trustedadvisor-account-level-read-access-denied, step task#1 (ListChecks GET /v1/checks) and step task#2 (ListRecommendations GET /v1/recommendations): observe 200 with no error payload instead of 403/AccessDeniedException.
7. All six must answer 403 with header x-amzn-errortype: AccessDeniedException and an AccessDeniedException body; re-replay both fixtures to confirm.
