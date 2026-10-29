Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# A ContextId that is neither an ARN nor * is accepted, approved and put in force on a resource plane that cannot exist

_Filed: 2026-10-27T22:21:24Z_
_Operation: RequestServiceQuotaIncrease | Class: validation_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ContextId` is the member that selects the RESOURCE plane of a quota. The model states what the value may be and what happens when it is not that: a resource-level increase whose ContextId ARN is invalid ends in the `INVALID_REQUEST` status. The handler performs exactly ONE check on `ContextId` — that the quota is adjustable per resource (`request_service_quota_increase.rs:174-210`) — and nothing at all on the value itself. Since the modelled `QuotaContextId` is a bare string with no `@pattern` and no `@length`, the boundary validator cannot refuse it either, so the handler is the only place the rule could be applied.

Consequence: `{"ServiceCode":"es","QuotaCode":<Instances per domain>,"DesiredValue":100,"ContextId":"not-an-arn"}` is accepted with HTTP 200, recorded at `QuotaRequestedAtLevel: RESOURCE`, walked through `CASE_OPENED` and settled to `APPROVED`, and the desired value is then written into the applied overlay under the key `(service, quota, Some("not-an-arn"))` (`state.rs:1152-1173`). From then on `GetServiceQuota`/`ListServiceQuotas` report a RESOURCE-level record whose `QuotaContext.ContextId` is `not-an-arn` — a resource plane for a resource that cannot exist — and `ListServiceQuotas` with `QuotaAppliedAtLevel=RESOURCE` enumerates it as a real applied quota. The documented `INVALID_REQUEST` status is unreachable: the crate declares constants only for PENDING, CASE_OPENED, APPROVED and NOT_APPROVED (`state.rs:155-173`) although its own generated value list carries INVALID_REQUEST.

`GetServiceQuota` has the same hole (`get_service_quota.rs:129-142` checks only `resource_adjustable`), so the phantom plane is also readable; both are the one missing check.

## Evidence

Model (smithy_query query="shape" shape="RequestedServiceQuotaChange", member `Status`): "`INVALID_REQUEST`: Service Quotas couldn't process your resource-level quota increase request because the Amazon Resource Name (ARN) specified as part of the `ContextId` is invalid."

Model (smithy_query query="doc" shape="RequestServiceQuotaIncreaseRequest" member="ContextId"): "Specifies the resource with an Amazon Resource Name (ARN)." and (shape="QuotaContextInfo", member `ContextId`): "The value for this field is either an Amazon Resource Name (ARN) or *."

Model (smithy_query query="shape" shape="QuotaContextId"): `{"shape_id": "com.amazonaws.servicequotas#QuotaContextId", "type": "string"}` — no pattern/length, so the framework's constraint walk cannot refuse any value and the handler is the only gate.

Handler — the only ContextId branch, `src/ops/request_service_quota_increase.rs:174-210`:
```
        match input.context_id.as_deref() {
            Some(context_id) if !resource_adjustable => { ... IllegalArgumentException ... }
            None if !account_adjustable => { ... }
            _ => {}
        }
```
and the value is then stored verbatim, `request_service_quota_increase.rs:283`: `input.context_id.clone(),`.

State keys the approved overlay by that same string, `src/state.rs:1152-1161`: `request.context_id.clone(),` inside the `(AppliedKey, AppliedQuota)` push — while `src/state.rs:469-471` states the intended domain: "the resource context the value was applied to — `None` for the account-wide value, `Some(id)` for the `ContextId` an increase named (an ARN, or [`CONTEXT_ID_EVERY_RESOURCE`])".

No INVALID_REQUEST anywhere in the crate's logic: `grep -n "INVALID_REQUEST" src/` matches only the generated enum list `src/ops/types/shared_types.rs:229` and two doc comments; `src/state.rs:155-173` declares STATUS_PENDING / STATUS_CASE_OPENED / STATUS_APPROVED / STATUS_NOT_APPROVED only.

Test coverage: every ContextId in the suite is an ARN or `*` — `tests/all/resource_quota_context.rs:81-84` (`fn domain(name) -> format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/{name}")`) and `CONTEXT_ID_EVERY_RESOURCE`; the one non-ARN ContextId used, `tests/all/quota_workflow.rs:1093-1111` (`"ContextId": "vpc-0123456789abcdef0"`), is refused for a different reason (the quota is account-only). Nothing pins acceptance of a non-ARN ContextId on a resource-adjustable quota, and no comment states a reason for it.

## Steps to Reproduce

1. Boot the emulator and resolve the `es` QuotaCode for "Instances per domain" (the catalog's one PER_RESOURCE/ALL row, src/state.rs:4039) via ListServiceQuotas.
2. POST `X-Amz-Target: ServiceQuotasV20190624.RequestServiceQuotaIncrease` with `{"ServiceCode":"es","QuotaCode":"<that code>","DesiredValue":100,"ContextId":"not-an-arn"}` -> HTTP 200, `RequestedQuota.Status = PENDING`, `QuotaRequestedAtLevel = RESOURCE`, `QuotaContext.ContextId = "not-an-arn"`.
3. Wait ~1.8 s (past SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY) and call GetRequestedServiceQuotaChange with the returned Id -> `Status = APPROVED`; the documented `INVALID_REQUEST` is never produced.
4. Call GetServiceQuota `{"ServiceCode":"es","QuotaCode":"<that code>","ContextId":"not-an-arn"}` -> HTTP 200 with `Value = 100` and a RESOURCE-level `QuotaContext` naming `not-an-arn`.
5. Call ListServiceQuotas `{"ServiceCode":"es","QuotaAppliedAtLevel":"RESOURCE"}` -> the phantom plane is enumerated as an applied quota.
