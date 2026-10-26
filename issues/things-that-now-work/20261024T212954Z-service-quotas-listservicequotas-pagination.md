Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# An unasked ListServiceQuotas page holds 10 quotas where AWS returns the service's whole applied-quota set

_Filed: 2026-10-24T21:29:54Z_
_Operation: ListServiceQuotas | Class: pagination_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A caller that names no `MaxResults` is served a 10-item page plus a `NextToken`, however many quotas the `ServiceCode` has.

Expected (AWS): `ListServiceQuotas` with only `ServiceCode` set returns the service's applied-quota set in one body for any realistically sized service — the same way the recorded AWS answer for the sibling default listing (`ListAWSDefaultServiceQuotas {"ServiceCode":"acm"}`) carries 44 `ServiceQuota` records in a single unasked response. `MaxResults` (@range 1..100) bounds what a CALLER asks for; the operation's own unasked default is far above 10.

Actual (emulator): the handler calls `crate::state::paginate(Self::NAME, quotas, input.max_results, input.next_token)` (src/ops/list_service_quotas.rs lines 161-175) and `paginate` substitutes `crate::state::UNASKED_PAGE_SIZE` = 10 (src/state.rs, constant declared ~line 203, applied in `paginate_bounded` ~line 1475) when `MaxResults` is absent. So a single-call client sees 10 of N quotas plus a body that claims more remains, where AWS returned the whole set — and for `ec2`/`lambda`/`vpc`, whose catalogs are long, it must now walk dozens of pages for a listing AWS answered once.

This is the same page-size decision already filed for `ListAWSDefaultServiceQuotas` (open issue 20261024T054353Z-service-quotas-listawsdefaultservicequotas-pagination) and for `ListServices` (fixed, filed separately per operation); that issue names `ListServiceQuotas` as sharing the constant but is scoped to the default-quota listing, so the applied listing is not covered by it.

## Evidence

Model: `ListServiceQuotas` is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Quotas)` with `MaxResults` `@range(1..100)` and no declared default (OPERATIONS.md lines 121-126).

Handler/state: src/ops/list_service_quotas.rs lines 161-175 (`crate::state::paginate(...)`); src/state.rs `UNASKED_PAGE_SIZE: usize = 10`, documented there as a CHOSEN value with nothing published fixing it, and used by `paginate` for every quota listing.

Recorded AWS behaviour for the sibling listing is quoted in the open catalog issue 20261023T182924Z (cause 3: an unasked `ListAWSDefaultServiceQuotas {"ServiceCode":"acm"}` body carries 44 quota records), which is what fixes the unasked page far above 10 for this service's quota listings.

Test coverage: tests/all/applied_quota_records.rs lines 504-575 and tests/all/quotas_catalog.rs lines 378-397 PIN the current 10-item unasked page for `ListServiceQuotas`, so those assertions must move with the fix.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.ListServiceQuotas` with body `{"ServiceCode":"lambda"}` (no `MaxResults`).
2. Count `$.Quotas`: exactly 10 records come back and `$.NextToken` is non-null, however many lambda quotas the catalog holds.
3. Walk the `NextToken` chain and count the total; compare with AWS, which answers the unasked call with the whole applied-quota set in one body and no `NextToken` for a service of that size.
