Issue-Source: trace-sweep-divergence
Issue-Trace: cost-optimization-hub-recommendation-summaries-workflow
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-optimization-hub-recommendation-summaries-workflow
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|ListRecommendationSummaries|2xx->2xx|$.metrics.savingsPercentage
Issue-Key: cost-optimization-hub|ListRecommendationSummaries|4xx->4xx|$.message
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# ListRecommendationSummaries metrics and filter-refusal responses diverge from AWS

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendationSummaries

## Description
Two causes in ListRecommendationSummaries' response construction: the savingsPercentage metric is reported as a number in a case where AWS reports a description instead, and the refusal of an unsupported filter does not say which filter was rejected.

Cause 1 of 2 — metrics.savingsPercentage returns a numeric figure where AWS returns the 'not supported' description (fixture cost-optimization-hub-recommendation-summaries-workflow, step task#2).
AWS: Smithy SummaryMetricsResult is documented as 'The results or descriptions for the additional metrics, based on whether the metrics were or were not requested', and its savingsPercentage member (type String) notes 'Savings percentage is only supported when filtering by Region, account ID, or tags.' Recorded behavior: for this request the field carries a sentence, not a figure — an example of the recorded text is 'Savings percentage is not supported when savings estimation mode preference is set to before discounts.' The requirement is the KIND of value: when the metric is not supported for the requested view, savingsPercentage carries a description of why no figure is reported, not a number.
Emulator: HTTP 200 with $.metrics.savingsPercentage = '0.00' — a formatted numeric percentage. The handler decides supportedness from the filter dimensions only (savings_percentage_is_defined) and never from the caller's effective savingsEstimationMode, so with BeforeDiscounts it computes and formats a figure (which also happens to be zero).
Why: a client that parses savingsPercentage reads 0.00 as a real measurement — 'these recommendations save 0% of my spend' — instead of learning the metric is unavailable for its current preference/view, so it reports a false zero on a dashboard or suppresses recommendations that do save money.
Fix: ListRecommendationSummaries' handler/metric computation — the supportedness test for savingsPercentage must also account for the effective savingsEstimationMode preference and return the description string in that case instead of a formatted number.
Traces: cost-optimization-hub-recommendation-summaries-workflow#task#2; services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_recommendation_summaries.rs:299-306, :343-344 and :385-414.

Cause 2 of 2 — the ValidationException for an unsupported filter does not identify the rejected filter (fixture cost-optimization-hub-recommendation-summaries-workflow, step task#3).
AWS: Smithy ListRecommendationSummaries documents 'The following filters are not supported for this API: recommendationIds, resourceArns, and resourceIds', and models ValidationException (HTTP 400). Status 400 and the ValidationException code match; what diverges is the top-level message, and its rule is known because the message is built from the request — it names the offending filter member and the rule that rejects it. Recorded example of that text: 'Invalid filter specified. Cannot include resourceId as a filter for this operation.'
Emulator: HTTP 400 ValidationException with $.message = 'Input validation failed. Please check the field list for more details.' — a fixed summary that names neither the member nor the rule; the diagnosis is pushed into the fields list by the service's shared field_validation_exception envelope.
Why: a client or operator reading the exception's message cannot tell which filter member was rejected, so the actionable part of the refusal is lost (SDK error surfaces and logs commonly show only message).
Fix: ListRecommendationSummaries' validation — the refusal for an unsupported filter member must put the member and the rule in the exception's message rather than only in fields. The requirement is that the message identifies the rejected filter member and that it is unsupported for this operation; the recorded wording above is an example, not the required string.
Traces: cost-optimization-hub-recommendation-summaries-workflow#task#3; services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_recommendation_summaries.rs:198-213, :434-439 and :465-473.

## Evidence
Fixture cost-optimization-hub-recommendation-summaries-workflow, trace set simparity-wire-aws-v1:

- task#2 ListRecommendationSummaries — expected/actual status 200; $.metrics.savingsPercentage expected the description 'Savings percentage is not supported when savings estimation mode preference is set to before discounts.', got '0.00'.
- task#3 ListRecommendationSummaries — expected/actual status 400 (ValidationException); $.message expected 'Invalid filter specified. Cannot include resourceId as a filter for this operation.', got 'Input validation failed. Please check the field list for more details.'

Model references (smithy_query on services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/smithy-model/com-amazonaws-costoptimizationhub-costoptimizationhubservice.json): SummaryMetricsResult doc 'The results or descriptions for the additional metrics, based on whether the metrics were or were not requested'; SummaryMetricsResult.savingsPercentage (String) note 'Savings percentage is only supported when filtering by Region, account ID, or tags'; SavingsEstimationMode enum [BeforeDiscounts, AfterDiscounts]; ListRecommendationSummaries doc 'The following filters are not supported for this API: recommendationIds, resourceArns, and resourceIds'; errors include ValidationException (HTTP 400).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture cost-optimization-hub-recommendation-summaries-workflow.
3. Step task#2 calls ListRecommendationSummaries requesting metrics=[SavingsPercentage] while the account's effective savingsEstimationMode is BeforeDiscounts: the emulator answers 200 with metrics.savingsPercentage='0.00' instead of the 'not supported' description the recording carries.
4. Step task#3 calls ListRecommendationSummaries with filter.resourceIds set: the emulator answers 400 ValidationException whose message is the generic 'Input validation failed...' instead of a message identifying the unsupported resourceId filter.
