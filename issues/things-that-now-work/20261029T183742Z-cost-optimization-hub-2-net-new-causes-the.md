Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cost-optimization-hub.list-recommendation-summaries-savings-percentage-metric-requested cost-optimization-hub.list-recommendations-filter-nonexistent-account-returns-empty
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|ListRecommendationSummaries|AccessDeniedException
Issue-Key: cost-optimization-hub.list-recommendations-filter-nonexistent-account-returns-empty|run#11|AssertionError|assert message == (
Issue-Commit: 57a21f5ddc052f99eaa78f0f6151a394ad8bec07

# cost-optimization-hub: 2 net-new causes — the recommendation readers' organization gate refuses a filter naming only the caller's own account, and ListRecommendations' denial message uses the wrong noun

> _Note: this issue was identified against the code base at commit `57a21f5ddc052f99eaa78f0f6151a394ad8bec07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendationSummaries

## Description
Two distinct net-new root causes in the cost-optimization-hub crate's shared organization-view gate for the recommendation readers. Both are in the same service; a fixer must close BOTH before submitting.

Related but NOT covering: open issue 20261029T180732Z-cost-optimization-hub-listrecommendations-maintainability--r3.md describes the SAME duplicated gate but declares it a pure maintainability/de-duplication finding and instructs that "both behaviours are pinned by tests and must stay exactly as they are" and the fix be "response-preserving". Cause 1 below shows one of those two pinned behaviours is itself the divergence, so that instruction (and the test that pins it, tests/all/shared_recommendation_view.rs:213-236) has to be revised rather than preserved. Open issue 20261029T043441Z-cost-optimization-hub-updatepreferences-validation--r2.md covers the unreachable AfterDiscounts mode and is NOT either cause here.

=== Cause 1 of 2: ListRecommendationSummaries (and ListRecommendations) refuse an accountIds filter that names only the caller's own account ===
Failing check: cost-optimization-hub.list-recommendation-summaries-savings-percentage-metric-requested

AWS: Filter$accountIds is documented only as "The account to which the recommendation applies." (Smithy: Filter$accountIds, a list of AccountId; ListRecommendationSummaries input filter: Filter, groupBy required; modelled errors AccessDeniedException @httpError 403, ValidationException 400). The model does not state the permission predicate, so for the admitted case this is recorded behavior only: on real AWS an ENROLLED account calling ListRecommendationSummaries with groupBy="ResourceType" and filter={"accountIds": [<its own account id>]} is served HTTP 200 with an items list (that call is the check's positive control, and the check's docstring records it as "a supported combination per the API docs"). AWS reserves AccessDeniedException for an accountIds scope that reaches accounts OTHER than the caller's — i.e. an organization view only the management account has. Naming one's own account is a read of one's own record, not an organization view.

Emulator: every populated filter.accountIds is refused, including a one-element list holding the caller's own account. The replayed call returns HTTP 403 AccessDeniedException with message "Cannot get recommendation summaries for member accounts of the organization. This is because your account is not the management account of the organization." (fixed text; the only variable part of this message class is the noun phrase naming the data, see Cause 2). The predicate is the inlined one at services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/state.rs:1186 — `if !named.is_empty() && !self.is_management_account(account_id)` — which, unlike the crate's other copy of the same gate (`addressed_accounts_for_read` -> `ReadScope::reaches_past`, state.rs:1087-1098 / 1281-1287), does not exclude a named scope that names only the caller. Because no code path in src/ ever records a management account, `is_management_account` is false for every wire caller, so NO client of the emulator can ever read recommendations or summaries with an accountIds filter at all — not even for itself.

Why: an accountIds filter scoped to the caller's own account is the only scope a non-management account has, and it is the normal way a client restricts a summary or recommendation read. Against the emulator that client gets a 403 permission error for data it owns, so it cannot read its own summaries, cannot distinguish a real permission problem from this one, and every downstream contract that call feeds (the metrics.savingsPercentage result, including the documented "Savings percentage is not supported when savings estimation mode preference is set to before discounts." sentinel) is unreachable and unverifiable.

Fix: the shared gate in the crate's state module — the recommendation readers' predicate in `recommendation_view` — must ask the same question the enrollment reader's gate asks (does the named scope reach PAST the caller?) rather than "is accountIds non-empty?", so a scope naming only the caller is admitted and answers that account's rows. The two callers (src/ops/list_recommendations.rs:221-224 and src/ops/list_recommendation_summaries.rs:274-277) keep their own modelled error for the genuinely-refused case.

=== Cause 2 of 2: ListRecommendations' organization-view AccessDeniedException carries the wrong fixed text ===
Failing check: cost-optimization-hub.list-recommendations-filter-nonexistent-account-returns-empty

AWS: recorded behavior only (the model gives AccessDeniedException no message text; Smithy: ListRecommendations errors AccessDeniedException @httpError 403). A member-account caller that passes any foreign accountIds to ListRecommendations is refused HTTP 403 AccessDeniedException whose message is, verbatim and entirely fixed text with no variable parts, "Cannot get enrollment statuses for member accounts of the organization. This is because your account is not the management account of the organization." — AWS words this refusal with the enrollment-statuses noun phrase for ListRecommendations too.

Emulator: the status (403) and the error code (AccessDeniedException) are right, but the message is "Cannot get recommendations for member accounts of the organization. This is because your account is not the management account of the organization." The second sentence matches; the first substitutes the noun "recommendations" for AWS's "enrollment statuses". The emulator parameterises this noun per operation (src/ops/list_recommendations.rs:384-388 passes the noun "recommendations" into the shared `organization_view_denied` helper in src/service.rs), which is exactly the part AWS does not vary.

Why: the message is part of the error contract. A client or test that matches this refusal on its message text (as the recorded check does) sees a different sentence from AWS's, so message-based classification, log assertions and user-facing surfacing of the denial all diverge even though the code and status agree.

Fix: the noun this operation hands to the shared organization-view-denied message builder in the service module — the refusal helper of the ListRecommendations handler — must produce AWS's fixed sentence for this refusal class rather than an operation-specific noun.

=== Traces ===
Cause 1: check cost-optimization-hub.list-recommendation-summaries-savings-percentage-metric-requested (errored: AccessDeniedException raised from ListRecommendationSummaries on the own-account positive control). Code sites: src/state.rs:1176-1195 (predicate at :1186), src/ops/list_recommendation_summaries.rs:274-277, src/ops/list_recommendations.rs:221-224; contrast src/state.rs:1087-1098 and :1281-1287.
Cause 2: check cost-optimization-hub.list-recommendations-filter-nonexistent-account-returns-empty (mismatch: message assertion). Code sites: src/ops/list_recommendations.rs:380-389, src/service.rs `organization_view_denied`.

## Evidence
Cause 1 — expected (real AWS, enrolled account, us-east-1):
  ListRecommendationSummaries{groupBy: "ResourceType", filter: {accountIds: [<caller's own account id>]}} -> HTTP 200, body carries items: [...] (a list), groupBy, currencyCode, and when metrics: ["SavingsPercentage"] is requested, metrics.savingsPercentage as a non-empty string.
Cause 1 — actual (emulator):
  HTTP 403, code AccessDeniedException, message "Cannot get recommendation summaries for member accounts of the organization. This is because your account is not the management account of the organization."
  Replay output: "botocore.errorfactory.AccessDeniedException: An error occurred (AccessDeniedException) when calling the ListRecommendationSummaries operation: Cannot get recommendation summaries for member accounts of the organization. This is because your account is not the management account of the organization."
  Source of the predicate — src/state.rs:1184-1188:
    // Refused for ANY populated `accountIds`, because what the caller lacks
    // is the view, not one account in it.
    if !named.is_empty() && !self.is_management_account(account_id) {
        return Err(OrganizationViewRequired);
    }
  The crate's other copy of the same gate admits a self-named scope — src/state.rs:1281-1287:
    fn reaches_past(&self, caller: &str) -> bool {
        match self { Self::Named(named) => named.iter().any(|wanted| wanted != caller), ... } }

Cause 2 — expected (real AWS, member-account caller, ListRecommendations with filter {accountIds: [<a 12-digit account outside the caller's organization>]}):
  HTTP 403, code AccessDeniedException, message exactly:
    "Cannot get enrollment statuses for member accounts of the organization. This is because your account is not the management account of the organization."
  (entirely fixed text; no request- or state-derived values appear in it)
Cause 2 — actual (emulator): HTTP 403, code AccessDeniedException, message:
    "Cannot get recommendations for member accounts of the organization. This is because your account is not the management account of the organization."
  Replay assertion failure: AssertionError: Cannot get recommendations for member accounts of the organization. This is because your account is not the management account of the organization.
  Source — src/ops/list_recommendations.rs:384-388:
    fn not_an_organization_view() -> ListRecommendationsError {
        ListRecommendationsErrorModeled::AccessDeniedException(AccessDeniedException {
            message: crate::service::organization_view_denied("recommendations"),
        }).into()
    }

Note on scope: the unfiltered ListRecommendations positive control and the ListEnrollmentStatuses / UpdateEnrollmentStatus prerequisites both succeeded against the emulator, so neither cause is an enrollment or warm-up artifact — the divergence is isolated to the accountIds gate and its message.

## Steps to Reproduce
Boot the emulator and point an SDK client for cost-optimization-hub at it (awsJson1_0, X-Amz-Target: CostOptimizationHubService.<Op>, region us-east-1).

Cause 1:
1. UpdateEnrollmentStatus{status: "Active"}; poll ListEnrollmentStatuses until items[0].status == "Active".
2. ListRecommendationSummaries{groupBy: "ResourceType", filter: {accountIds: [<the caller's own account id, e.g. from sts:GetCallerIdentity>]}}.
   Observe HTTP 403 AccessDeniedException ("Cannot get recommendation summaries for member accounts of the organization. ...") instead of HTTP 200 with an items list.
3. Repeat with metrics: ["SavingsPercentage"] — same 403, so metrics.savingsPercentage can never be observed at all.
4. ListRecommendations{filter: {accountIds: [<own account id>]}} — same refusal from the same shared gate.
5. Re-replay check cost-optimization-hub.list-recommendation-summaries-savings-percentage-metric-requested to confirm.

Cause 2:
1. Enroll as above.
2. ListRecommendations{maxResults: 10} with no filter — succeeds (positive control).
3. ListRecommendations{filter: {accountIds: ["111122223333"]}, maxResults: 10} — observe HTTP 403 AccessDeniedException (correct code/status) but message "Cannot get recommendations for member accounts of the organization. ..." instead of AWS's "Cannot get enrollment statuses for member accounts of the organization. This is because your account is not the management account of the organization."
4. Re-replay check cost-optimization-hub.list-recommendations-filter-nonexistent-account-returns-empty to confirm.
