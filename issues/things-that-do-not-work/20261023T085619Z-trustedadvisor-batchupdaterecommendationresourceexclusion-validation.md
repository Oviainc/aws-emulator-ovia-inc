Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 5a6a50abfee118ac44c2855d86c20698e89c1a34

# A batch naming the same recommendation-resource ARN twice is accepted and silently last-wins

_Filed: 2026-10-23T08:56:19Z_
_Operation: BatchUpdateRecommendationResourceExclusion | Class: validation_

> _Note: this issue was identified against the code base at commit `5a6a50abfee118ac44c2855d86c20698e89c1a34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the operation is documented to support "up to 25 unique recommendation resource ARNs per request", i.e. each ARN may appear at most once; a batch that names one ARN twice with conflicting `isExcluded` values is not a well-formed request and AWS refuses it (400 ValidationException) rather than picking one of the two states.

Actual: the modelled `@uniqueItems` on `RecommendationResourceExclusionList` only de-duplicates whole entries, so `[{arn: X, isExcluded: true}, {arn: X, isExcluded: false}]` passes the constraint walk. The handler then iterates entries in request order and writes `resource.exclusion_status` for each, so the LAST occurrence silently wins, the request answers 200 with an empty `batchUpdateRecommendationResourceExclusionErrors`, and the caller is never told its batch named one ARN twice with contradictory intent. Nothing in the handler tracks the ARNs already seen in this request.

## Evidence

Smithy model: operation doc for BatchUpdateRecommendationResourceExclusion — "This API supports up to 25 unique recommendation resource ARNs per request"; `RecommendationResourceExclusionList` is `@length(1,25) @uniqueItems` over `RecommendationResourceExclusion{arn, isExcluded}`, so uniqueness is enforced per STRUCTURE, not per ARN; ValidationException (HTTP 400) is bound to the operation. Handler: services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/batch_update_recommendation_resource_exclusion.rs, `for exclusion in &input.recommendation_resource_exclusions { ... resource.exclusion_status = if exclusion.is_excluded {..} else {..} }` (~lines 211-263) — no duplicate-ARN check. Tests: tests/all/validation_message_rendering.rs exercises only the list length bounds (26/30 entries, empty list); no test covers a repeated ARN (grep for 'duplicate' in tests/ returns no match).

## Steps to Reproduce

1. Boot the emulator with an account entitled to the Trusted Advisor API.
2. PUT /v1/batch-update-recommendation-resource-exclusion with body {"recommendationResourceExclusions": [{"arn": "arn:aws:trustedadvisor::<account>:recommendation-resource/amazon-ebs-snapshots-recommendation/res-ebs-0001", "isExcluded": true}, {"arn": "<the same ARN>", "isExcluded": false}]}.
3. Observe HTTP 200 with an empty error list; GET /v1/recommendations/<the owning recommendation ARN>/resources shows the resource at `included` (the last entry won).
4. Expected: the request naming one recommendation-resource ARN twice is refused with 400 ValidationException and the stored exclusion status is unchanged.
