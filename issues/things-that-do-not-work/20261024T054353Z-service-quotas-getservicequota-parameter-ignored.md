Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# ContextId never selects a resource-scoped quota and QuotaContext is never reported

_Filed: 2026-10-24T05:43:53Z_
_Operation: GetServiceQuota | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetServiceQuota` accepts a `ContextId` but can never honour it: the member is only ever used to REFUSE the call, never to resolve a resource-scoped quota value, and the `ServiceQuota.QuotaContext` structure is hard-coded absent on every response.

Expected (AWS): for a quota that is adjustable per resource, `GetServiceQuota(ServiceCode, QuotaCode, ContextId=<resource ARN or *>)` returns the value applied to THAT resource, with `QuotaAppliedAtLevel = RESOURCE` and a populated `QuotaContext { ContextScope, ContextScopeType, ContextId, AdjustableAtLevel }` (Smithy `QuotaContextInfo`, whose doc describes exactly this resource-level plane). `ListServiceQuotas(QuotaAppliedAtLevel=RESOURCE)` then reports those records.

Actual (emulator): every catalog row is built with `adjustable_at_level: LEVEL_ACCOUNT` (`src/state.rs` line 341), so `src/ops/get_service_quota.rs` lines 124-137 answers ANY `ContextId` with `IllegalArgumentException`; the lookup at line 139 (`applied_quota_view`) never receives the `ContextId` at all, and both quota views set `quota_context: None` (`src/state.rs` line 1121 for `ServiceQuota`, line 838 for `RequestedServiceQuotaChange`). Consequences a client observes: `QuotaContext` is absent from every `GetServiceQuota`/`ListServiceQuotas`/`GetAWSDefaultServiceQuota` record; `ListServiceQuotas(QuotaAppliedAtLevel="RESOURCE")` is always an empty list; `RequestServiceQuotaIncrease` with a `ContextId` is always refused and its stored request is always `QuotaRequestedAtLevel = ACCOUNT` (`src/state.rs` line 691), so the resource-level half of the API surface is unreachable.

## Evidence

Model: `GetServiceQuotaRequest.ContextId` (`QuotaContextId`) exists and is documented as naming the resource the quota applies to; `ServiceQuota.QuotaContext : QuotaContextInfo` and `AppliedLevelEnum` values `ACCOUNT | ALL | RESOURCE` are modeled (smithy_query shape `ServiceQuota`, `QuotaContextInfo`), as is `RequestedServiceQuotaChange.QuotaContext`. `QuotaContextInfo`'s own doc gives the worked AWS example ("Instances per OpenSearch Service Domain" applied per domain).

Handler/state: `src/ops/get_service_quota.rs` lines 122-139 — `account_level_only` is true for every seeded quota, `ContextId` is rejected, and `applied_quota_view(&region, &service_code, &quota_code)` takes no context argument. `src/state.rs` line 341 hard-codes `adjustable_at_level: LEVEL_ACCOUNT` for every catalog row; lines 1120-1121 and 838 hard-code `quota_applied_at_level`/`quota_context` to `None`.

Test coverage: `tests/all/quota_workflow.rs` lines 1091-1109 assert only the refusal for an ACCOUNT-only quota; no test exercises a resource-scoped quota, and nothing in the suite can, since the catalog contains no quota adjustable at RESOURCE/ALL level. This is distinct from the open catalog-content issue (`20261023T182924Z-...catalog-is-incomplete-and`), which lists missing rows and wrong ServiceNames but not the context/level dimension, and the limitation is in the state and handler layers, not only in the seed data.

## Steps to Reproduce

1. Boot the emulator and POST `ListServiceQuotas` `{"ServiceCode":"ec2","QuotaAppliedAtLevel":"RESOURCE"}` -> an empty `Quotas` list; no record in any listing carries a `QuotaContext` member.
2. POST `GetServiceQuota` `{"ServiceCode":"vpc","QuotaCode":"L-F678F1CE","ContextId":"arn:aws:ec2:us-east-1:000000000000:vpc/vpc-0123456789abcdef0"}` -> HTTP 400 `IllegalArgumentException`; the same refusal comes back for every ServiceCode/QuotaCode pair in the catalog, so no `ContextId` value is ever honoured.
3. POST `RequestServiceQuotaIncrease` with the same `ContextId` -> HTTP 400; without it, the stored request reports `QuotaRequestedAtLevel: ACCOUNT` and no `QuotaContext`.
