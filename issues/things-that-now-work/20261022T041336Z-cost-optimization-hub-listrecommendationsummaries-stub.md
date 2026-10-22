Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cost-optimization-hub.list-recommendation-summaries-group-by-resource-type-structure cost-optimization-hub.list-recommendation-summaries-savings-percentage-metric-requested
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|ListRecommendationSummaries|NotImplemented
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# Implement cost-optimization-hub ListRecommendationSummaries: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendationSummaries

## Description
`ListRecommendationSummaries` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_recommendation_summaries.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListRecommendationSummaries` call, so each one unblocks only when `ListRecommendationSummaries` works: `cost-optimization-hub.list-recommendation-summaries-group-by-resource-type-structure`, `cost-optimization-hub.list-recommendation-summaries-savings-percentage-metric-requested`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_recommendation_summaries.rs`. Replaying the corpus, the emulator refused the `ListRecommendationSummaries` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListRecommendationSummaries operation: operation ListRecommendationSummaries is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cost-optimization-hub emulator
Call ListRecommendationSummaries and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
