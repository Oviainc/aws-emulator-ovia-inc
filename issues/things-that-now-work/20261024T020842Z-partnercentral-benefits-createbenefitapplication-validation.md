Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Tags supplied at create bypass the per-resource tag cap that TagResource enforces

_Filed: 2026-10-24T02:08:42Z_
_Operation: CreateBenefitApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBenefitApplication` stores the whole `Tags` list verbatim (`crate::state::tag_map(input.tags.as_ref())`) with no cap, while `TagResource` refuses any merge that would take a resource past `MAX_TAGS_PER_RESOURCE` with the modelled `ServiceQuotaExceededException`. A create carrying more tags than that cap is accepted with HTTP 200, and `ListTagsForResource` then reports a tag set larger than the service claims it permits — and no `TagResource` call could ever have produced it.

Expected: the per-resource tag limit is one property of the resource, enforced on every path that adds tags; a create whose `Tags` exceed it is refused (ValidationException / ServiceQuotaExceededException, matching the limit's own error) rather than silently stored.

Actual: only the tag-mutating path enforces the limit; the create path does not, so the limit is bypassable and the stored tag set is unbounded by anything.

## Evidence

Handler: services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/create_benefit_application.rs line ~331 `tags: crate::state::tag_map(input.tags.as_ref())` — no count check. Contrast src/ops/tag_resource.rs lines ~215-231, which computes the merged set and answers `ServiceQuotaExceededException` (modelled on TagResource/UntagResource, HTTP 402) when `merged.len() > crate::state::MAX_TAGS_PER_RESOURCE`.
Spec: the `Tags` list shape carries no @length (smithy_query(shape='Tags')), so the bound is business logic, which the emulator does implement — just not on the create path; `ListTagsForResource` makes the oversized set observable.
Test coverage: tests/all/resource_tagging.rs / resource_tags.rs exercise the quota only through TagResource; no test creates an application with an oversized Tags list.

## Steps to Reproduce

1. Grant the partner entitlement.
2. Call CreateBenefitApplication(Catalog='Sandbox', ClientToken='t0', BenefitIdentifier=<seeded benefit id>, Name='demo', Tags=[{Key:'k0',Value:'v'}, ... more entries than the service's per-resource tag limit]).
3. Observe HTTP 200.
4. Call ListTagsForResource(resourceArn=<the created application's Arn>) and observe a tag set larger than the limit TagResource enforces.
5. Call TagResource on the same ARN with one extra tag: it is refused with ServiceQuotaExceededException, proving the limit the create ignored.
