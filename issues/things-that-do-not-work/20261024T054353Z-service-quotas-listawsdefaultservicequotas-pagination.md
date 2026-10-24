Issue-Severity: HIGH
Issue-Gap-Class: pagination
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# An unasked ListAWSDefaultServiceQuotas page holds 10 quotas where AWS returns the service's whole default set

_Filed: 2026-10-24T05:43:53Z_
_Operation: ListAWSDefaultServiceQuotas | Class: pagination_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A caller that names no `MaxResults` is served a 10-item page plus a `NextToken`, no matter how many default quotas the `ServiceCode` has.

Expected (AWS): `ListAWSDefaultServiceQuotas` with only `ServiceCode` set returns the service's default-quota set in one response for any realistically sized service — the recorded AWS answer for `{"ServiceCode":"acm"}` carries 44 `ServiceQuota` records in a single body, and for `{"ServiceCode":"access-analyzer"}` 11 records. The service's documented `MaxResults` default is "a value appropriate to the operation"; the recordings fix that value far above 10.

Actual (emulator): `crate::state::UNASKED_PAGE_SIZE` is 10 (`src/state.rs` line 202, used by `paginate` at line 1409), so the handler (`src/ops/list_aws_default_service_quotas.rs` lines 141-155) answers an unasked call with the first 10 quotas of the service and a `NextToken`. Once the catalog is complete, every single-call client sees 10 of N quotas and a body that claims there is more, where AWS returned the whole set.

The same constant governs `ListServiceQuotas`, `ListRequestedServiceQuotaChangeHistoryByQuota` and `ListServices` (filed separately for `ListServices`), so one page-size decision diverges across every listing of this service. `GetQuotaUtilizationReport` is NOT affected: it goes through `paginate_bounded` with its own documented default of 1,000.

## Evidence

Model: `ListAWSDefaultServiceQuotas` is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Quotas)` with `MaxResults` `@range(1..100)` and no declared default (OPERATIONS.md, lines 93-98).

Handler: `src/ops/list_aws_default_service_quotas.rs` lines 141-155 calls `crate::state::paginate(Self::NAME, quotas, input.max_results, ...)`; `src/state.rs` lines 1452-1454 substitutes `UNASKED_PAGE_SIZE` (10, declared at line 202 and documented there as a CHOSEN value with nothing published to fix it) when `max_results` is `None`.

Recorded AWS behaviour, quoted in the still-open issue `20261023T182924Z-service-quotas-default-quota-catalog-is-incomplete-and`: its cause 3 states the recorded body for `ListAWSDefaultServiceQuotas {"ServiceCode":"acm"}` (repro step 4 shows the request carries no `MaxResults`) contains 44 QuotaCodes, and cause 1 the same for 11 access-analyzer QuotaCodes — i.e. AWS's unasked page is at least 44 items, not 10. That issue is scoped to catalog CONTENT (missing/extra rows, wrong ServiceNames) and says nothing about page size, so this is not covered by it.

Test coverage: `tests/all/default_quota_catalog.rs` lines 361-371 and `tests/all/quotas_catalog.rs` lines 378-386 assert the current 10-item unasked page, so the divergence is pinned by tests and those assertions must move with the fix.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.ListAWSDefaultServiceQuotas` with body `{"ServiceCode":"ec2"}` (no `MaxResults`).
2. Count `$.Quotas`: the response carries 10 records and a non-null `$.NextToken`, regardless of how many ec2 default quotas the catalog holds.
3. Compare with the recorded AWS answer for an unasked call (e.g. `{"ServiceCode":"acm"}` returns 44 records in one body with no NextToken).
