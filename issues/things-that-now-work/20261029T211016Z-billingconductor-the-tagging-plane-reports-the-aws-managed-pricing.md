Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billingconductor.tag-operations-nonexistent-resource-not-found
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor.tag-operations-nonexistent-resource-not-found|run#5|AssertionError|assert control_code != "NotFoundException", (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# The tagging plane reports the AWS-managed pricing plans (Passthrough / BasicPricingPlan) as NotFoundException, so every tagging existence check is vacuous

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
REOPENS 20261022T200837Z-billingconductor-tagresource-stub.md: its fix left the call refused. That issue carries this exact check id (Issue-Trace-Ids: billingconductor.tag-operations-nonexistent-resource-not-found) and implemented the tagging surface, but the check now fails one step earlier, on a 404 refusal of its positive-control ListTagsForResource call. This is not a new stub; it is the tagging plane's existence decision refusing a resource that exists.

Cause 1 of 1: the tagging plane resolves a ResourceArn ONLY against resources this emulator's caller created, so an AWS-OWNED pricing plan reads as not-found.

AWS: billingconductor's pricing-plan namespace contains plans Amazon Web Services owns rather than the caller. The Smithy model's pricing-plan ARN form allows the literal `aws` account segment beside a 12-digit account and enumerates the managed ids (`.../pricingplan/(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})`), which is why a billing group may name one without the caller ever having created it — the crate itself states this on `AWS_MANAGED_PRICING_PLAN_IDS` (state.rs). Those plans are therefore always-present resources of every account, and a tagging call that names one addresses an EXISTING resource. Smithy: ListTagsForResource / TagResource / UntagResource (uri `/tags/{ResourceArn}`, ResourceArn @required @httpLabel) each model `ResourceNotFoundException` @httpError 404 ("The request references a resource that doesn't exist"); recorded behavior for this service's tagging plane is HTTP 404 with error code `NotFoundException` and message "Resource was not found" (the crate records that recording in `TAGGING_NOT_FOUND_CODE` / `TAGGING_NOT_FOUND_STATUS` / `TAGGING_NOT_FOUND_MESSAGE`). Whatever AWS answers for a tagging call against an AWS-managed plan, it is NOT that not-found: the resource is there. Hypothesis (neither modeled nor recorded): a WRITE to an AWS-owned plan is refused as AccessDenied/Validation rather than accepted — only the not-found classification is the requirement here.

Emulator: `ListTagsForResource(ResourceArn="arn:aws:billingconductor::aws:pricingplan/Passthrough")` is answered HTTP 404, error code `NotFoundException`, message "Resource was not found". In that message, "Resource was not found" is the whole fixed text — the recorded message names nothing from the request — so the divergence is not the wording but that it is served at all for a resource that exists. The cause: `ListTagsForResource::handle` resolves existence through `crate::state::resource_tags`, which expands `resource_tags_of!` -> `parse_taggable_arn` -> a plain `pricing_plans.get(&resource.arn)`. The AWS-managed plans are never records in `pricing_plans`, so the lookup returns `None` and the handler returns `crate::state::tagging_not_found()`. The crate already knows these ARNs elsewhere (`is_aws_managed_pricing_plan`, used by the pricing-plan association path), so the tagging plane is the one door that does not honour them. TagResource and UntagResource share the same resolution (`resource_tags_mut`) and so share the misclassification.

Why: a client gets two wrong answers from one defect. (1) An existing, AWS-managed pricing plan — the only pricing plan present before the caller creates one, and the only one a non-payer account can reach at all, since CreatePricingPlan is payer-gated — reports as not-found, so a client using the tagging plane to confirm a resource exists, or to read/clear tags on the default plan, is told the plan does not exist. (2) Because the emulator answers NotFoundException for EVERY ResourceArn it has no record for, with no notion of resources it does not own, its tagging surface is indistinguishable from one that simply always refuses: every negative assertion about a bogus ARN passes vacuously, so a client (or test) cannot rely on the tagging plane's not-found to mean anything.

Fix: the tagging resolution in state.rs — `parse_taggable_arn` and the `resource_tags_of!` dispatch behind `resource_tags` / `resource_tags_mut` — must resolve the AWS-managed pricing plans as existing resources, the same way the pricing-plan association path already does via `is_aws_managed_pricing_plan`, rather than treating absence from the `pricing_plans` map as non-existence. With that, ListTagsForResource reports the managed plan's (empty) tag set instead of 404/NotFoundException, and TagResource/UntagResource stop classifying an AWS-owned plan as not-found. The not-found path itself (code, status, message) is correct and must stay as `tagging_not_found()` has it for ARNs that really name no resource.

Traces: billingconductor.tag-operations-nonexistent-resource-not-found. Code: services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/list_tags_for_resource.rs:171-173 (`resource_tags` -> `tagging_not_found`); src/state.rs:2490-2512 (`resource_tags_of!` pricing-plan arm), :3399-3409 (`resource_tags` / `resource_tags_mut`), :3604-3640 (`parse_taggable_arn`), :28 and :641-645 (`AWS_MANAGED_PRICING_PLAN_IDS`, `is_aws_managed_pricing_plan` — the recognition the tagging plane does not use), :3977-3995 (the recorded not-found code/status/message).

## Evidence
Failing check: billingconductor.tag-operations-nonexistent-resource-not-found (verdict: mismatch).

Expected (AWS): a tagging call naming the AWS-managed default pricing plan `arn:aws:billingconductor::aws:pricingplan/Passthrough` must not be classified not-found — that plan is an AWS-owned resource the pricing-plan ARN form explicitly admits (account segment `aws`, id `Passthrough`), present without the caller creating it.

Actual (emulator): ListTagsForResource on that ARN answers HTTP 404, Error.Code `NotFoundException`, Error.Message "Resource was not found". The check's own positive control trips on it:

  AssertionError: positive control: a real, AWS-managed pricing plan must not read as not-found (NotFoundException: Resource was not found)

The control exists precisely to prove the emulator is checking existence rather than answering not-found unconditionally; it failing means the three negative assertions that follow (TagResource / ListTagsForResource / UntagResource on a bogus pricingrule ARN must each be NotFoundException + "Resource was not found") would have been satisfied vacuously.

Mechanism, from the checkout: ops/list_tags_for_resource.rs calls `state.resource_tags(&input.resource_arn)` and returns `crate::state::tagging_not_found()` on `None`; `resource_tags` expands `resource_tags_of!`, whose PricingPlan arm is `state.pricing_plans.get(&resource.arn)`. No AWS-managed plan is ever a record in `pricing_plans`, while state.rs separately declares `AWS_MANAGED_PRICING_PLAN_IDS = ["BasicPricingPlan", "Passthrough"]` and `is_aws_managed_pricing_plan(arn)` for the association path — so the two doors onto pricing plans disagree about which plans exist.

Smithy (services/service-com-amazonaws-billingconductor-awsbillingconductor/smithy-model/com-amazonaws-billingconductor-awsbillingconductor.json): ListTagsForResource GET /tags/{ResourceArn}, errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); output `Tags` (TagMap).

## Steps to Reproduce
1. Boot the billingconductor emulator.
2. Call ListTagsForResource with ResourceArn="arn:aws:billingconductor::aws:pricingplan/Passthrough" (the AWS-managed default pricing plan; also try "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan").
3. Observe HTTP 404 with Error.Code=NotFoundException and Error.Message="Resource was not found", although that plan is an AWS-owned resource the service's pricing-plan ARN form admits and the crate's own AWS_MANAGED_PRICING_PLAN_IDS names.
4. Call TagResource / UntagResource on the same ARN and observe the same 404 NotFoundException from the shared resolution.
5. Call any of the three tagging operations with a syntactically valid but nonexistent ARN (e.g. arn:aws:billingconductor::<account>:pricingrule/BOGUS0000A) and observe the identical 404 NotFoundException — showing the plane answers not-found without distinguishing resources it does not own from resources that do not exist.
6. Replay billingconductor.tag-operations-nonexistent-resource-not-found to see the positive control fail.
