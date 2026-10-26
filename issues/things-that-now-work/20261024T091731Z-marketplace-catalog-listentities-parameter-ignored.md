Issue-Severity: HIGH
Issue-Gap-Class: parameter-ignored
Issue-Commit: 671706971a680016eadecf6b3438021fa34d8a56

# Honor EntityTypeFilters / EntityTypeSort instead of refusing the request with 422

_Filed: 2026-10-24T09:17:31Z_
_Operation: ListEntities | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `671706971a680016eadecf6b3438021fa34d8a56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEntitiesRequest$EntityTypeFilters` ("A Union object containing filter shapes for all EntityTypes ... that can be used to search or filter entities") and `EntityTypeSort` are the modern, documented way to filter/sort a ListEntities call (e.g. `AmiProductFilters.ProductTitle`, `OfferFilters.BuyerAccounts`, `EntityTypeSort.AmiProductSort`).

The handler refuses any request that carries either one: `src/ops/list_entities.rs` returns a `ValidationException` (HTTP 422) with the message "this emulator lists entities by FilterList and Sort; the type-specific <member> is not implemented". A valid, modeled request therefore fails outright instead of returning the filtered/sorted page the real service returns.

Expected: `EntityTypeFilters` selects entities on the named type's own members and `EntityTypeSort` orders them (at minimum the members the store already holds — `EntityId`, `ProductTitle`/`Name`, `Visibility`, `LastModifiedDate` — with unsupported sub-members treated as no-ops rather than fatal), returning HTTP 200. Actual: HTTP 422 ValidationException for every request that names either member.

## Evidence

Spec: `ListEntitiesRequest$EntityTypeFilters` and `$EntityTypeSort` are modeled input members (smithy_query on ListEntitiesRequest; the union `EntityTypeFilters` has AmiProductFilters/OfferFilters/... each with `EntityId`, `LastModifiedDate`, `ProductTitle`, `Visibility` sub-filters that this store already holds). Handler: `src/ops/list_entities.rs` lines ~202-218 return ValidationException whenever `input.entity_type_filters.is_some() || input.entity_type_sort.is_some()`. Tests: `tests/all/change_set_lifecycle.rs:1532-1559` asserts the 422 — i.e. the current test enshrines the divergence and must be updated when this is fixed.

## Steps to Reproduce

1. Create an AmiProduct entity via StartChangeSet (Intent=APPLY, Details {"ProductTitle":"titled"}) and wait for SUCCEEDED
2. POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"AmiProduct","EntityTypeFilters":{"AmiProductFilters":{"EntityId":{"ValueList":["<entity-id>"]}}}}
3. Observe HTTP 422 ValidationException naming EntityTypeFilters; expected HTTP 200 with EntitySummaryList containing that entity
4. Repeat with {"EntityTypeSort":{"AmiProductSort":{"SortBy":"EntityId","SortOrder":"ASCENDING"}}} — observe 422 instead of an ascending page
