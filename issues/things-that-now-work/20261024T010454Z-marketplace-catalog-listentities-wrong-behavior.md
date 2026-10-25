Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# ListEntities refuses EntityTypeFilters/EntityTypeSort with 422 and never reports the non-product per-type summaries

_Filed: 2026-10-24T01:04:54Z_
_Operation: ListEntities | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two divergences in the same list projection:

1. `EntityTypeFilters` and `EntityTypeSort` are documented, first-class members of `ListEntitiesRequest` ("A Union object containing filter shapes for all EntityTypes. Each EntityTypeFilter shape will have filters applicable for that EntityType that can be used to search or filter entities"). Real AWS accepts them and returns 200 with the filtered/ordered page (this is the modern way to filter by e.g. `AmiProductFilters.Visibility`, `.ProductTitle`, `.LastModifiedDate`, since the generic `FilterList` only supports `EntityId`). The emulator answers 422 `ValidationException` whenever either member is present, so any caller using the type-specific filter/sort surface gets a client error instead of results.

2. For entity types outside the five product types, `EntitySummary` never carries its per-type summary: `OfferSummary`, `OfferSetSummary`, `ResaleAuthorizationSummary` and `ExpressPrivateOfferConfigurationSummary` are hardcoded to `None` even when the listed entity is of that type. Real AWS fills the summary member matching `EntityType` (e.g. an `Offer` entity reports `OfferSummary` with `Name`, `ProductId`, `State`, …).

## Evidence

Spec: `ListEntitiesRequest$EntityTypeFilters` / `$EntityTypeSort` are modeled members with documentation describing them as the per-EntityType search/filter surface; `EntitySummary` models `OfferSummary`/`OfferSetSummary`/`ResaleAuthorizationSummary`/`ExpressPrivateOfferConfigurationSummary`.
Handler: `src/ops/list_entities.rs` lines ~196-218 return `ValidationException` with the message "this emulator lists entities by FilterList and Sort; the type-specific {member} is not implemented".
State: `src/state.rs` `EntityRecord::to_summary()` sets `offer_summary: None, resale_authorization_summary: None, offer_set_summary: None, express_private_offer_configuration_summary: None` with the comment "The summaries of the remaining entity types carry members this slice does not model".
Tests: `tests/all/change_set_lifecycle.rs` lines 1532-1564 assert the 422 refusal, i.e. the divergence is pinned by a test rather than fixed.

## Steps to Reproduce

1. StartChangeSet with a change that creates an `AmiProduct` entity and wait for the change set to reach SUCCEEDED
2. POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"AmiProduct","EntityTypeFilters":{"AmiProductFilters":{"Visibility":{"ValueList":["Limited"]}}}}
3. Observe 422 ValidationException naming EntityTypeFilters; AWS returns 200 with the matching entities
4. Repeat with "EntityTypeSort":{"AmiProductSort":{"SortBy":"EntityId","SortOrder":"ASCENDING"}} — same 422
5. Create an `Offer` entity via StartChangeSet, then POST /ListEntities with EntityType=Offer and observe the returned EntitySummary carries no OfferSummary member
