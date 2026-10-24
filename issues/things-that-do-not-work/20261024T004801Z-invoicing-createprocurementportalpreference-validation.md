Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# CreateProcurementPortalPreference accepts more ResourceTags than the per-resource tag quota the tag plane enforces

_Filed: 2026-10-24T00:48:01Z_
_Operation: CreateProcurementPortalPreference | Class: validation_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ResourceTags` on this create is modelled `@length(0..200)`, but the emulator's per-resource tag quota is 50 (`crate::state::MAX_RESOURCE_TAGS`, derived from the AWS-wide "maximum 50 tags per resource" quota and enforced atomically with 402 ServiceQuotaExceededException by TagResource, and with ValidationException by CreateInvoiceUnit). The create handler stores whatever tag list arrives without consulting that quota, so a create carrying 51-200 distinct keys succeeds and leaves a preference holding more tags than any subsequent TagResource call would be allowed to put there.

Expected: a create whose `ResourceTags` would push the new resource past the per-resource tag quota is refused before anything is persisted — this operation models `ServiceQuotaExceededException` (HTTP 402), the same error the tag plane answers for the quota, so the refusal has a modelled home here.

Actual: 200 ResourceTags on a create are accepted; ListTagsForResource then reports e.g. 120 tags on the preference, while adding even one more tag through TagResource is refused 402 — the quota is observably inconsistent between the create path and the tag path, and the over-quota resource could never be reproduced against real AWS.

## Evidence

Handler: services/service-com-amazonaws-invoicing-invoicing/src/ops/create_procurement_portal_preference.rs lines ~349-358 — the input tags are collected into a BTreeMap and inserted into `store.resource_tags` with no quota check.

Contrast inside the same crate: services/service-com-amazonaws-invoicing-invoicing/src/ops/tag_resource.rs:163-193 refuses atomically with `ServiceQuotaExceededException(TAG_QUOTA_EXCEEDED_MESSAGE)` once `held + added > MAX_RESOURCE_TAGS`, and services/service-com-amazonaws-invoicing-invoicing/src/ops/create_invoice_unit.rs:195-214 applies the same bound on its own create path. services/service-com-amazonaws-invoicing-invoicing/src/state.rs documents MAX_RESOURCE_TAGS = 50 as a quota "on the resource, independent of how many tags one call may carry", refused "ATOMICALLY with the modelled ServiceQuotaExceededException (HTTP 402)".

Spec: smithy_query(query="operation", operation="CreateProcurementPortalPreference") — `ResourceTags` is `ResourceTagList` @length(0..200) and the operation declares `ServiceQuotaExceededException (HTTP 402)`. The 50-tags-per-resource quota is established AWS tagging behaviour and is already the emulator's own stated rule.

Test coverage: tests/all/procurement_portal_preference*.rs exercise creates with small tag lists only; no test asserts the quota on this create path.

## Steps to Reproduce

1. Call CreateProcurementPortalPreference for a fresh portal/buyer/supplier identity with ResourceTags containing 60 distinct keys.
2. Observe HTTP 200 and an ARN. Expected: HTTP 402 ServiceQuotaExceededException ("The request exceeds the maximum number of tags allowed per resource.") with nothing persisted.
3. Call ListTagsForResource on the returned ARN — observe 60 tags on the resource.
4. Call TagResource on the same ARN with one more key — observe 402 ServiceQuotaExceededException, showing the create bypassed the quota the tag plane enforces.
