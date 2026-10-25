Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: trustedadvisor.batch-update-resource-exclusion-exceeds-max-items-rejected trustedadvisor.get-recommendation-matches-list-summary trustedadvisor.list-checks-pagination-no-duplicates trustedadvisor.list-recommendations-check-identifier-filter-consistency trustedadvisor.list-recommendations-status-partition-invariant trustedadvisor.update-recommendation-lifecycle-full-cycle-transitions trustedadvisor.update-recommendation-lifecycle-short-reason-rejected
Issue-Shape-Id: com.amazonaws.trustedadvisor#TrustedAdvisor
Issue-Key: trustedadvisor.batch-update-resource-exclusion-exceeds-max-items-rejected|run#15|AssertionError|assert code1 == "AccessDeniedException", (
Issue-Key: trustedadvisor.get-recommendation-matches-list-summary|_error_code#3|AssertionError|raise AssertionError("expected a ClientError, got a success response")
Issue-Key: trustedadvisor.list-checks-pagination-no-duplicates|_error#5|AssertionError|raise AssertionError(f"expected this call to be rejected, but it succeeded with {result!r}")
Issue-Key: trustedadvisor.list-recommendations-check-identifier-filter-consistency|_assert_denied_for_support_level#5|AssertionError|raise AssertionError(
Issue-Key: trustedadvisor.list-recommendations-status-partition-invariant|run#3|AssertionError|raise AssertionError(
Issue-Key: trustedadvisor.update-recommendation-lifecycle-full-cycle-transitions|run#6|AssertionError|assert error["Code"] == "AccessDeniedException", (
Issue-Key: trustedadvisor.update-recommendation-lifecycle-short-reason-rejected|run#8|AssertionError|assert control_code == "AccessDeniedException", (
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# trustedadvisor: support-level entitlement gate absent — checks/recommendations surface answers 200 or 404 where AWS answers 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendations

## Description
The emulator does not model the Trusted Advisor account entitlement (support-plan) gate at all. The recorded account holds no Trusted Advisor-eligible support plan (Business Support+/Enterprise/Unified Operations), and on real AWS every bound Check/Recommendation operation is therefore refused with AccessDeniedException after request validation but BEFORE any identifier is resolved against state. The emulator instead evaluates each request on its merits: read/list and batch-mutation calls succeed with HTTP 200, and identifier-scoped calls resolve against empty state and return HTTP 404 ResourceNotFoundException.

NOTE — SAME ROOT CAUSE AS AN ALREADY-OPEN ISSUE: 20261024T152002Z-trustedadvisor-support-level-accessdeniedexception-never-returned-by-any.md describes this identical gap from the WIRE corpus (simparity-wire-ga-prod-v1). That issue carries only wire replay keys, so it does not carry any of the behavioral check ids below; the replay suite did not mark any of these checks OWNED. This issue exists to attach the behavioral replay keys to the same fix. The two trustedadvisor stub issues (GetOrganizationRecommendation, ListOrganizationRecommendationResources) cover Organization-scoped operations and do NOT cover this.

This is ONE defect with ONE cause; it must be closed once, and all seven behavioral checks in trace_ids must then pass.

Cause 1 of 1: the entitlement/authorization decision is missing from the trustedadvisor request path, for both reads and mutations, and is not evaluated before resource resolution.

AWS: Smithy (services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json) binds AccessDeniedException (errorKind client, @httpError 403, documentation "Exception that access has been denied due to insufficient access") to every operation exercised here: ListChecks (GET /v1/checks), ListRecommendations (GET /v1/recommendations), GetRecommendation (GET /v1/recommendations/{recommendationIdentifier}), UpdateRecommendationLifecycle (PUT /v1/recommendations/{recommendationIdentifier}/lifecycle) and BatchUpdateRecommendationResourceExclusion (PUT /v1/batch-update-recommendation-resource-exclusion). For an unentitled account the recorded behavior is HTTP 403 with error code AccessDeniedException on every one of those calls. Recorded message, as an EXAMPLE only (no model or doc rule produces it): "Access denied due to support level".

Ordering is part of the recorded contract and is independently evidenced: constraint validation still runs and still wins. The recordings show ValidationException/400 for a 26-item recommendationResourceExclusions list (Smithy: RecommendationResourceExclusionList @length min 1 max 25), for pillar='not_a_real_pillar' (RecommendationPillar enum), for status='not-a-real-status' (RecommendationStatus enum) and for an updateReason below 10 characters (RecommendationUpdateReason @length min 10 max 4096) — and the emulator already reproduces all four of those correctly. Only once the request is well-formed does the entitlement denial fire, and it fires before existence is consulted: UpdateRecommendationLifecycle and GetRecommendation on a syntactically valid but nonexistent recommendation ARN (AccountRecommendationIdentifier pattern ^arn:[\\w-]+:trustedadvisor::\\d{12}:recommendation\\/[\\w-]+$) answer 403, not 404. Note BatchUpdateRecommendationResourceExclusion does not even model ResourceNotFoundException, so 403 is the only denial available to it.

Emulator: ListChecks(maxResults=200) and ListChecks(maxResults=1) return HTTP 200 with a populated checkSummaries catalogue; ListRecommendations(maxResults=1) and ListRecommendations(maxResults=100) return HTTP 200 (the latter with 4 recommendationSummaries from seeded state); BatchUpdateRecommendationResourceExclusion with a single well-formed exclusion returns HTTP 200 and accepts the mutation; UpdateRecommendationLifecycle and GetRecommendation with a well-formed reason/identifier return ResourceNotFoundException (observed message example: "no recommendation with the identifier arn:aws:trustedadvisor::<account>:recommendation/<id>") instead of AccessDeniedException/403.

Why: clients branch on the error code. An unentitled caller is told it is entitled-but-empty, so (a) a Trusted Advisor UI or remediation job reports "nothing to remediate" where AWS refuses to answer at all — the most damaging possible misreading of this API; (b) the upgrade/fallback path a client takes on AccessDeniedException is never taken; (c) on BatchUpdateRecommendationResourceExclusion it is an authorization bypass, not merely a wrong code — a write AWS forbids is reported as having succeeded, so the client's exclusion state diverges permanently from the service's; and (d) a denial is indistinguishable from a missing resource, so callers retry other identifiers or conclude their local state is stale.

Fix: one shared account-entitlement (support-level) gate in the trustedadvisor service crate's request dispatch, applied to every bound Check/Recommendation operation — reads and mutations alike — and ordered AFTER input constraint validation but BEFORE identifier resolution or any state write, emitting AccessDeniedException serialized as HTTP 403. Belongs in the service's dispatch/authorization layer rather than per-operation handlers, since all five operations above diverge identically; the existing validation path must not be moved behind it or the four passing ValidationException assertions regress.

Traces (behavioral check ids, all in service trustedadvisor): list-checks-pagination-no-duplicates, list-recommendations-check-identifier-filter-consistency, list-recommendations-status-partition-invariant, get-recommendation-matches-list-summary, batch-update-resource-exclusion-exceeds-max-items-rejected, update-recommendation-lifecycle-full-cycle-transitions, update-recommendation-lifecycle-short-reason-rejected.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1: 13/20 passed (65%); all 7 failures are trustedadvisor, all 7 are [mismatch], and all 7 fail on the same expectation — HTTP 403 / error code AccessDeniedException for a well-formed call by an unentitled account.

1. trustedadvisor.list-checks-pagination-no-duplicates — the invalid-pillar ValidationException assertion PASSED, then ListChecks(maxResults=200) was expected to be rejected and instead succeeded: HTTPStatusCode 200 with a full checkSummaries list (amazon-ebs-snapshots, iam-access-key-rotation, low-utilization-ec2-instances, ...). AssertionError: "expected this call to be rejected, but it succeeded with {...'HTTPStatusCode': 200...}".

2. trustedadvisor.list-recommendations-check-identifier-filter-consistency — AssertionError: "list_checks: expected Trusted Advisor to deny this account on support level ...; the call succeeded instead" (ListChecks(maxResults=1) -> 200 instead of 403/AccessDeniedException).

3. trustedadvisor.list-recommendations-status-partition-invariant — the invalid-status ValidationException positive control PASSED, then AssertionError: "ListRecommendations unexpectedly succeeded on an account with no Trusted Advisor API entitlement; got 4 recommendation(s)" (expected 403/AccessDeniedException).

4. trustedadvisor.get-recommendation-matches-list-summary — AssertionError at the first ListRecommendations(maxResults=1): "expected a ClientError, got a success response" (expected AccessDeniedException with HTTPStatusCode 403).

5. trustedadvisor.batch-update-resource-exclusion-exceeds-max-items-rejected — the 26-item oversized-batch ValidationException (400, exact 25-item-max message) PASSED, proving validation works; the same-shape SINGLE-item batch then returned success. AssertionError: "expected the account's Trusted Advisor entitlement gate (AccessDeniedException) for a same-shape single-item batch, got 'OK' (None)" — expected 403; a forbidden mutation was accepted with 200.

6. trustedadvisor.update-recommendation-lifecycle-full-cycle-transitions — AssertionError: "expected AccessDeniedException for lifecycleStage='in_progress', got 'ResourceNotFoundException' ('no recommendation with the identifier arn:aws:trustedadvisor::000000000000:recommendation/deadbeefdeadbeefdeadbeefdeadbeef')" — expected 403 AccessDeniedException; got 404 ResourceNotFoundException, i.e. existence resolved before the entitlement decision.

7. trustedadvisor.update-recommendation-lifecycle-short-reason-rejected — the too-short updateReason ValidationException (exact 10-char-minimum message) PASSED; swapping in a valid-length reason on the identical call then gave AssertionError: "expected AccessDeniedException (support-plan gate) once the reason clears the length check, got 'ResourceNotFoundException'".

Cross-check against the wire corpus: open issue 20261024T152002Z-trustedadvisor-support-level-accessdeniedexception-never-returned-by-any.md records the same 403->200 divergence for ListChecks, ListRecommendations and BatchUpdateRecommendationResourceExclusion and the same 403->404 divergence for GetRecommendation, ListRecommendationResources and UpdateRecommendationLifecycle. The behavioral evidence above adds the ordering constraint: validation already precedes the (missing) gate correctly, so only the gate itself is absent.

Model support: AccessDeniedException is bound as a client error with httpError 403 on all five operations exercised; BatchUpdateRecommendationResourceExclusion models no ResourceNotFoundException at all.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from it.
2. Run the behavioral suite (run_behavioral_suite, no arguments) — trace set simparity-behavioral-ga-prod-v1.
3. Observe the 7 trustedadvisor failures listed in trace_ids. Equivalent manual reproduction with a botocore 'trustedadvisor' client against the emulator:
   a. ListChecks(maxResults=200) and ListChecks(maxResults=1) -> must each be HTTP 403 with error code AccessDeniedException; emulator returns 200 with a checkSummaries catalogue.
   b. ListRecommendations(maxResults=1) and ListRecommendations(maxResults=100) -> must each be HTTP 403 / AccessDeniedException; emulator returns 200 (4 recommendationSummaries).
   c. BatchUpdateRecommendationResourceExclusion with ONE well-formed exclusion (arn:aws:trustedadvisor::<account>:recommendation-resource/<uuid>/<uuid>, isExcluded=true) -> must be HTTP 403 / AccessDeniedException; emulator returns 200 and accepts the write. With 26 such items it must stay HTTP 400 / ValidationException (this already works — do not regress it).
   d. GetRecommendation(recommendationIdentifier='arn:aws:trustedadvisor::<account>:recommendation/00000000-0000-0000-0000-000000000000') -> must be HTTP 403 / AccessDeniedException even though no such recommendation exists; emulator returns 404 / ResourceNotFoundException. With a malformed identifier it must stay HTTP 400 / ValidationException.
   e. UpdateRecommendationLifecycle on the same nonexistent-but-well-formed ARN with lifecycleStage in (in_progress, dismissed, pending_response) and an updateReason of 10+ characters -> must be HTTP 403 / AccessDeniedException for every stage; emulator returns 404 / ResourceNotFoundException. With updateReason='short' (parameter_validation disabled client-side) it must stay HTTP 400 / ValidationException.
4. Apply the fix: a shared support-level entitlement gate in the trustedadvisor dispatch path, after input validation and before identifier resolution / state writes, returning AccessDeniedException as HTTP 403.
5. Re-replay the behavioral suite and confirm all 7 checks in trace_ids pass and the 13 previously passing checks still pass; also re-replay the wire fixtures trustedadvisor-account-level-read-access-denied and trustedadvisor-account-level-mutation-access-denied from the related open issue.
