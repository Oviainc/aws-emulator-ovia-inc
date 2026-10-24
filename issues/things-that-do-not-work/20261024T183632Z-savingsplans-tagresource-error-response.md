Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# TagResource never produces its modeled ServiceQuotaExceededException for the per-resource tag quota

_Filed: 2026-10-24T18:36:32Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource declares four errors in the Smithy model: InternalServerException (500), ResourceNotFoundException (404), ServiceQuotaExceededException (402, "A service quota has been exceeded.") and ValidationException (400). The emulator can produce three of them, but never the 402: the only quota TagResource can exceed is the per-resource tag count, and the handler answers that with a 400 ValidationException.

Expected: a TagResource call whose merged tag set would take the Savings Plan past the service's per-resource tag quota (50 tags) is refused as a QUOTA failure — HTTP 402 with x-amzn-errortype: ServiceQuotaExceededException — leaving the stored tag set unchanged. Shape violations (an over-long tag key or value, which are constraint failures, not quota failures) stay 400 ValidationException.

Actual: every over-limit write is answered HTTP 400 ValidationException ("a Savings Plan cannot carry more than 50 tags, got N"), so the modeled 402 is unreachable on this operation and a client that branches on ServiceQuotaExceededException (e.g. to back off and prune tags) never sees it. Note the asymmetry the model itself draws: UntagResource and ListTagsForResource, which cannot exceed a quota, do NOT declare ServiceQuotaExceededException, while TagResource does — the quota error is modeled precisely because a tag write can exceed one.

## Evidence

Model: smithy_query(query="operation", operation="TagResource") lists ServiceQuotaExceededException with httpError 402 for TagResource; UntagResource/ListTagsForResource do not declare it. The per-resource limit of 50 tags is an AWS-documented quota ("Tag naming limits and requirements", AWS General Reference), and the emulator itself documents it as such: services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs lines 456-494 (MAX_TAGS_PER_PLAN = 50, check_tags).

Handler: src/ops/tag_resource.rs lines 162-166 — plan.merge_tags(input.tags).map_err(|message| TagResourceErrorModeled::ValidationException(ValidationException { message }))? — the only failure channel for the tag-count quota is ValidationException; the ServiceQuotaExceededException variant generated in TagResourceErrorModeled (lines ~88-95) is never constructed anywhere in the crate (grep for ServiceQuotaExceededException shows it used only in create_savings_plan.rs and return_savings_plan.rs).

Test coverage: tests/all/savings_plans_lifecycle.rs::tag_writes_stop_at_the_tag_limit (lines 1247-1266) asserts (400, "ValidationException") for "one more key than the limit allows" and "a whole request over the limit at once" — i.e. the current 400 is locked in by a test; no test asserts the modeled 402 for a quota-exceeding tag write. Precedent: the same class of gap was filed and fixed for ReturnSavingsPlan ("ReturnSavingsPlan never produces its modeled ServiceQuotaExceededException for the documented per-month return limits").

## Steps to Reproduce

1. Boot the emulator and call POST /DescribeSavingsPlansOfferings to pick an offering id, then POST /CreateSavingsPlan with that savingsPlanOfferingId and commitment "0.1"; read the plan's savingsPlanArn from POST /DescribeSavingsPlans.
2. Fill the plan to the per-resource tag quota: call POST /TagResource 50 times with one distinct tag key each (or once with a 50-entry tags map). All answer 200.
3. Call POST /TagResource once more with {"resourceArn": <arn>, "tags": {"one-more": "v"}}.
4. Observe HTTP 400 with x-amzn-errortype: ValidationException; the quota-exceeding write should be reported as the operation's modeled ServiceQuotaExceededException (HTTP 402).
5. Call POST /ListTagsForResource to confirm the refused write stored nothing (this part is already correct — only the error code/status is wrong).
