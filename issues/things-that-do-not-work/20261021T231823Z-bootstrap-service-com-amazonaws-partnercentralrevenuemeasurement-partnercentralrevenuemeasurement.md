Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the PartnerCentral Revenue Measurement emulator

_Filed: 2026-10-21T23:18:23Z_
_Operation: service-com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 18 operations in this crate is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:`) answering HTTP 501 `NotImplemented` and holding no state, so no client workflow works at all.

Implement a FIRST SMALL SLICE: the Marketplace Revenue Share lifecycle, which is this service's primary resource and the only self-contained create -> use -> read -> list -> update loop in the model. Six operations, in order:

1. **CreateMarketplaceRevenueShare** — input `Catalog` (AWS|Sandbox, required), `ProductId` (`^prod-[a-z0-9]{13}$`, required), `ClientToken` (@idempotencyToken), `Tags`. Must persist a new revenue share keyed by (Catalog, ProductId) and return `ProductId`, `Arn` (built as `arn:aws:partnercentral:<region>:<account>:catalog/<Catalog>/marketplace-revenue-share/<ProductId>`), `Catalog`, `ProductCode`, `ProductName`, `CreatedDate`, `LastModifiedDate`, `Revision` = 1. Re-create with the same `ClientToken` must be idempotent (@idempotent); a second create for an existing ProductId with a different token must fail `ConflictException` (409).
2. **GetMarketplaceRevenueShare** — input `Catalog`, `ProductId`, optional `Revision`. Must look the record up in state and return exactly what was created, plus the COMPUTED fields `LatestRevision`, `TotalMarketplaceRevenueShareAllocationCount` and `TotalActiveMarketplaceRevenueShareAllocationCount` derived by counting the stored allocations (active = `Status == ACTIVE`). Unknown ProductId -> `ResourceNotFoundException` (404).
3. **CreateMarketplaceRevenueShareAllocation** — input `Catalog`, `ProductId` (required, must exist else 404), `EffectiveFrom` (`^\d{4}-\d{2}-\d{2}$`, required), `EffectiveUntil`, `RevenueSharePercent` (`^\d{1,3}(\.\d{1,2})?$`, required), `ClientToken`. Must mint a server-side `MarketplaceRevenueShareAllocationId` matching `^mrsa-[A-Za-z0-9]{13}$`, store it under the parent share, bump the parent's revision, and return the stored allocation with `Arn`, `Status` = ACTIVE, `ProductName`, `CreatedDate`, `LastModifiedDate`, `LatestMarketplaceRevenueShareRevision`.
4. **GetMarketplaceRevenueShareAllocation** — input `Catalog`, `ProductId`, `MarketplaceRevenueShareAllocationId`, optional `MarketplaceRevenueShareRevision`. Must return the persisted allocation field-for-field (same `EffectiveFrom`/`EffectiveUntil`/`RevenueSharePercent`/`Status` that were written); unknown id -> `ResourceNotFoundException`.
5. **ListMarketplaceRevenueShareAllocations** — input `Catalog`, `ProductId` (required), filters `Status`, `AfterEffectiveFrom`, `BeforeEffectiveFrom`, `SortBy` (EffectiveFrom), `SortOrder`, `MaxResults`, `NextToken`. Must return `MarketplaceRevenueShareAllocationSummaries` built from stored allocations, honour the `Status` and effective-date filters, honour `MaxResults` by truncating, and emit an opaque `NextToken` (matching `^[\S]+$`) that resumes exactly where the page ended; omit `NextToken` on the last page.
6. **UpdateMarketplaceRevenueShareAllocation** — input `Catalog`, `ProductId`, `MarketplaceRevenueShareAllocationId`, `MarketplaceRevenueShareRevision` (required), optional `EffectiveFrom`, `EffectiveUntil`, `RevenueSharePercent`, `Status`. Must mutate the stored allocation in place (only supplied fields), update `LastModifiedDate`, bump the parent share revision, and return the NEW values — a following Get and List must observe the change, and the active-allocation count from step 2 must change when `Status` flips to INACTIVE.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input back, no minting an id that is never stored. `ListMarketplaceRevenueShares` is nice-to-have in the slice (listing the shares created in step 1 with `ProductIds`/`ProductCodes`/`CreatedAfter`/`CreatedBefore` filters and pagination) if it falls out of the same store cheaply.

All input constraints in the model should be enforced with `ValidationException` (400): the `prod-`/`mrsa-` id patterns, the `^\d{4}-\d{2}-\d{2}$` date strings, the `RevenueSharePercent` numeric pattern, the `CatalogName` enum (AWS|Sandbox), and `ClientToken` length 1..64.

## State Model

In `src/state.rs`, on the service state struct reached via `crate::state::service_state(&*self.engine)`:

- `marketplace_revenue_shares: HashMap<(CatalogName, String /* ProductId */), MarketplaceRevenueShare>`
- `MarketplaceRevenueShare { product_id: String, arn: String, catalog: CatalogName, product_code: String, product_name: String, created_date: SystemTime, last_modified_date: SystemTime, revision: i32, tags: Vec<Tag>, allocations: IndexMap<String /* mrsa-id */, MarketplaceRevenueShareAllocation> }`
- `MarketplaceRevenueShareAllocation { allocation_id: String /* mrsa-… */, arn: String, product_id: String, effective_from: String, effective_until: Option<String>, revenue_share_percent: String, status: AllocationStatus /* ACTIVE|INACTIVE */, created_date: SystemTime, last_modified_date: SystemTime, latest_marketplace_revenue_share_revision: String }`
- `idempotency: HashMap<String /* ClientToken */, String /* resource id created */>` so @idempotent re-sends return the first result instead of creating a duplicate.
- `next_id_seed: u64` (or an RNG) used ONLY to mint `prod-`/`mrsa-` suffixes that are then written into the maps above.

Derived values (`LatestRevision`, `TotalMarketplaceRevenueShareAllocationCount`, `TotalActiveMarketplaceRevenueShareAllocationCount`, list summaries, pagination tokens) must be computed from these structures at read time, never stored as a literal in the response builder.

The remaining 12 operations (the RevenueAttribution family — Create/Get/Update/List, GetRevenueAttributionAllocation, ListRevenueAttributionAllocations, Start/GetRevenueAttributionAllocationsTask — and TagResource/UntagResource/ListTagsForResource) stay at 501 for now and will be filed as individual gap issues.

Done = an integration test that drives steps 1-6 against the emulator and asserts the response bodies (ids, dates, percent, status, counts, pagination) reflect what was written, not just 2xx.

## Evidence

OPERATIONS.md at the crate root lists all 18 operations with their input/output members, routes (`POST /CreateMarketplaceRevenueShare` etc.) and errors; the field names, patterns (`^prod-[a-z0-9]{13}$`, `^mrsa-[A-Za-z0-9]{13}$`, `^\d{4}-\d{2}-\d{2}$`, `^\d{1,3}(\.\d{1,2})?$`), enums (CatalogName AWS|Sandbox, AllocationStatus ACTIVE|INACTIVE) and @paginated/@idempotent traits cited above come from it. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 18 handler files (create_marketplace_revenue_share.rs, get_marketplace_revenue_share.rs, create_marketplace_revenue_share_allocation.rs, get_marketplace_revenue_share_allocation.rs, list_marketplace_revenue_share_allocations.rs, update_marketplace_revenue_share_allocation.rs, …), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No tests exercise any operation; only the generated tests/state_roundtrip.rs exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at the `partnercentral-prm` endpoint.
2. CreateMarketplaceRevenueShare(Catalog="AWS", ProductId="prod-abc1234567890", ClientToken="tok-1") — expect 200 with Arn, Revision=1; today returns HTTP 501 NotImplemented.
3. GetMarketplaceRevenueShare(Catalog="AWS", ProductId="prod-abc1234567890") — expect the same Arn/CreatedDate created above and allocation counts of 0.
4. CreateMarketplaceRevenueShareAllocation(Catalog="AWS", ProductId="prod-abc1234567890", EffectiveFrom="2024-01-01", RevenueSharePercent="12.5") — expect 200 with a minted MarketplaceRevenueShareAllocationId matching ^mrsa-[A-Za-z0-9]{13}$ and Status=ACTIVE.
5. GetMarketplaceRevenueShareAllocation with that id — expect EffectiveFrom="2024-01-01" and RevenueSharePercent="12.5" returned from state.
6. ListMarketplaceRevenueShareAllocations(Catalog="AWS", ProductId="prod-abc1234567890", MaxResults=1) — expect exactly one summary for the allocation just created and a NextToken only if more remain.
7. UpdateMarketplaceRevenueShareAllocation(same ids, MarketplaceRevenueShareRevision=<current>, RevenueSharePercent="20", Status="INACTIVE") — expect 200 with the new values.
8. Re-run GetMarketplaceRevenueShareAllocation — expect RevenueSharePercent="20", Status=INACTIVE; re-run GetMarketplaceRevenueShare — expect TotalMarketplaceRevenueShareAllocationCount=1 and TotalActiveMarketplaceRevenueShareAllocationCount=0.
9. GetMarketplaceRevenueShare(Catalog="AWS", ProductId="prod-zzz0000000000") — expect ResourceNotFoundException (404), not 501.
