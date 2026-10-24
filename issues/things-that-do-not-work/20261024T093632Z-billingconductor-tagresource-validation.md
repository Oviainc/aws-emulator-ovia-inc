Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# TagResource stores tag keys in the reserved aws: namespace instead of refusing them

_Filed: 2026-10-24T09:36:32Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler merges every supplied key/value pair into the resource's tag map after only checking the per-resource tag cap. Keys using the AWS-reserved `aws:` prefix are accepted and persisted.

Expected: `aws:`-prefixed tag keys are reserved for AWS's own use; a customer request that creates or adds such a key is refused with a `ValidationException` (HTTP 400), and the resource's tags are unchanged.

Actual: `POST /tags/{ResourceArn}` with `{"Tags":{"aws:cost-center":"x"}}` answers 204 and a following `GET /tags/{ResourceArn}` reports `aws:cost-center`, which also makes the reserved key count against the per-resource cap and removable by `UntagResource`. The same hole exists on the create paths that persist `Tags` (`CreateBillingGroup`, `CreatePricingPlan`, `CreatePricingRule`, `CreateCustomLineItem` all do `input.tags.unwrap_or_default().into_iter().collect()`), so a reserved key can also enter the store at create time.

## Evidence

Spec/well-known behaviour: the model's `TagKey` carries only `@length(1..128)` and no pattern (smithy_query query="shape" shape="TagKey"), so the boundary walk cannot refuse it; AWS's tagging rules reserve the `aws:` prefix service-wide — you cannot create, edit or delete `aws:`-prefixed tags, and a request that tries is rejected with a validation error. The service does gate tagging on resource resolution rather than the payer gate (`src/service.rs` PAYER_UNGATED_OPERATIONS), so this path is reachable by any caller.

Handler: `src/ops/tag_resource.rs` lines ~173-207 — resolve the ARN, check `MAX_TAGS_PER_RESOURCE`, then `for (key, value) in input.tags { stored.insert(key, value); }` with no key inspection.

Tests: `tests/all/tagging_surface.rs` covers merge semantics, selective untag, the 200-tag cap and the 404 for unknown resources; no test exercises a reserved `aws:` key, so nothing pins the current acceptance.

## Steps to Reproduce

1. Create a pricing plan and note its ARN (tagging needs no payer gate, but the create does — use the crate's payer seam)
2. POST /tags/arn:aws:billingconductor::<account>:pricingplan/<id> with {"Tags":{"aws:cost-center":"reserved","team":"ok"}}
3. Observe 204 instead of a 400 ValidationException naming the reserved key
4. GET /tags/arn:aws:billingconductor::<account>:pricingplan/<id> — the response reports the `aws:cost-center` key alongside `team`
5. Equivalently, POST /create-pricing-plan with {"Name":"p","Tags":{"aws:foo":"bar"}} and read the tags back: the reserved key is stored
