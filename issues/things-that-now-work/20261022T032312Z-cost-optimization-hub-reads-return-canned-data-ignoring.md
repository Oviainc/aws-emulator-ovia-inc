Issue-Source: trace-sweep-divergence
Issue-Trace: cost-optimization-hub-list-recommendations-filter-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-optimization-hub-list-recommendations-filter-errors cost-optimization-hub-preferences-configure-and-verify
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|ListRecommendations|2xx->2xx|$.items[*]
Issue-Key: cost-optimization-hub|GetPreferences|2xx->2xx|$
Issue-Commit: f81229b87ab2500fc3bfee437baf8654766d98fe

# Cost Optimization Hub reads return canned data, ignoring request filters and stored preferences

> _Note: this issue was identified against the code base at commit `f81229b87ab2500fc3bfee437baf8654766d98fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendations

## Description
Two read operations answer 200 with a fixed payload instead of one derived from the request and from state the preceding steps established: ListRecommendations returns a recommendation that the supplied filter excludes, and GetPreferences returns an empty object after UpdatePreferences succeeded. Two recorded causes, one root cause: the handlers do not consult request input / service state.

Cause 1 of 2: ListRecommendations ignores the `filter` member.
AWS: Smithy: ListRecommendations.filter -> Filter, documented as 'The constraints that you want all returned recommendations to match', with dimension members (accountIds, regions, resourceTypes, actionTypes, resourceIds, resourceArns, recommendationIds, implementationEfforts, tags, restartNeeded, rollbackPossible); output items -> RecommendationList. Every item returned must satisfy the filter, so a filter matching nothing yields items=[] with HTTP 200. The recording shows HTTP 200 with an empty items list for this filtered request.
Emulator: HTTP 200 but items[0] is present — a LambdaFunction 'Rightsize' recommendation that does not satisfy the request's filter — i.e. the filter is not applied and the full canned recommendation set is returned.
Why: a client that filters by account, region, resource or action type and then acts on each item (surfacing savings for one account, or driving a rightsizing workflow) receives out-of-scope resources and attributes savings/actions to the wrong scope; a 'no matching recommendations' path is never exercised because the list is never empty.
Fix: the ListRecommendations handler — apply each populated Filter member as a conjunctive predicate over the recommendation set before serializing items (and before paginating).
Traces: cost-optimization-hub-list-recommendations-filter-errors#task#2.

Cause 2 of 2: GetPreferences returns an empty body and does not reflect UpdatePreferences.
AWS: Smithy: GetPreferences takes no input and returns GetPreferencesResponse{savingsEstimationMode, memberAccountDiscountVisibility, preferredCommitment}; it 'Returns a set of preferences for an account', and UpdatePreferences 'Updates a set of preferences for an account', echoing the same members. So after a successful UpdatePreferences the following GetPreferences must return the stored values. The recording shows HTTP 200 with preferredCommitment={paymentOption:'PartialUpfront', term:'OneYear'} and the savingsEstimationMode the fixture had set.
Emulator: HTTP 200 with an entirely empty JSON object — none of the three members is returned, so the preferences written by the earlier UpdatePreferences step are not persisted or not read back.
Why: a client that sets a commitment/savings-estimation preference and then reads it back sees nothing, so configuration appears to silently fail; read-after-write on account preferences is unusable and code branching on savingsEstimationMode or preferredCommitment takes the unset path.
Fix: the GetPreferences handler plus the service state that UpdatePreferences writes — store the preference structure per account and serialize all set members on read.
Traces: cost-optimization-hub-preferences-configure-and-verify#task#4.

Values in the recordings (recommendation ids, ARNs, timestamps, savings figures, the specific filter values) are examples; the requirements are that items satisfy the filter and that GetPreferences echoes what UpdatePreferences stored. No open issue covers either operation (no existing cost-optimization-hub issues are filed).

## Evidence
Cause 1 — fixture cost-optimization-hub-list-recommendations-filter-errors step task#2, ListRecommendations, expected HTTP 200 / actual HTTP 200: $.items[0] is `extra` — expected None, got {recommendationId:'0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0004', accountId:'000000000000', region:'us-west-2', resourceId:'invoice-renderer', currentResourceType:'LambdaFunction', actionType:'Rightsize', estimatedMonthlySavings:47.6, source:'ComputeOptimizer', tags:[{key:'environment', value:'staging'}], ...}. The recorded response for the same filtered request has no items.

Cause 2 — fixture cost-optimization-hub-preferences-configure-and-verify step task#4, GetPreferences, expected HTTP 200 / actual HTTP 200: $ is `empty_body` — expected {preferredCommitment:{paymentOption:'PartialUpfront', term:'OneYear'}, savingsEstimationMode:<the value set earlier in the fixture>}, got {}. The preceding UpdatePreferences step in the same fixture passed.

Model consulted: services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/smithy-model/com-amazonaws-costoptimizationhub-costoptimizationhubservice.json (ListRecommendations, Filter, ListRecommendationsRequest, GetPreferences, GetPreferencesResponse, UpdatePreferences). Handlers live under services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/ with state in .../src/state.rs.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture cost-optimization-hub-list-recommendations-filter-errors, step task#2: ListRecommendations with a `filter` that matches none of the seeded recommendations. Observe HTTP 200 with a non-empty items list containing a recommendation outside the filter; expected items=[].
3. Replay fixture cost-optimization-hub-preferences-configure-and-verify: UpdatePreferences with preferredCommitment={paymentOption:'PartialUpfront', term:'OneYear'} (step passes), then step task#4 GetPreferences. Observe HTTP 200 with body {}; expected the stored preferredCommitment and savingsEstimationMode.
