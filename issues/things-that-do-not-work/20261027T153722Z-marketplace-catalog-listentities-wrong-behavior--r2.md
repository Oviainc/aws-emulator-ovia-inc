Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0d4ec79976a0b91eeb5a23059b2beecdecae287a

# ListEntities refuses six EntityType values the model documents as valid

_Filed: 2026-10-27T15:37:22Z_
_Operation: ListEntities | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0d4ec79976a0b91eeb5a23059b2beecdecae287a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEntities` validates the required `EntityType` member against the crate's own `ENTITY_TYPES` constant, a nine-entry set derived from `EntitySummary`'s per-type summary members. The Smithy documentation of `ListEntitiesRequest$EntityType` itself enumerates the valid values, and six of them — `ProcurementPolicy`, `Experience`, `Audience`, `BrandingSettings`, `Seller`, `Solution` — are absent from `ENTITY_TYPES`. A request naming one of those documented values is refused before any listing with a 422 `ValidationException` saying `Requested entity type 'Seller' is invalid.`, instead of being listed (an empty page while the catalog holds no entity of that type, which is what the handler's own comment says a known-but-empty type answers).

The same constant gates `crate::state::is_entity_type_reference` (used by `StartChangeSet` for `Change$Entity$Type`) and `may_carry_policy`, so the gap also keeps a change set from ever creating an entity of those documented types, but the model sentence that proves the defect is the `ListEntitiesRequest$EntityType` doc.

## Evidence

Model (smithy_query query="doc" shape="ListEntitiesRequest" member="EntityType"), verbatim: "The type of entities to retrieve. Valid values are: AmiProduct, ContainerProduct, DataProduct, SaaSProduct, ProcurementPolicy, Experience, Audience, BrandingSettings, Offer, OfferSet, Seller, ResaleAuthorization, Solution."

Handler, services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_entities.rs:186-196:
```
        // `EntityType` is checked against the types this catalog HAS, not only
        // against the characters its `@pattern` admits: a letters-only value
        // naming no type of this catalog names nothing to list, so it is refused
        // before any listing is performed rather than answered as an empty page,
        if !crate::state::is_entity_type(&input.entity_type) {
            return Err(ListEntitiesErrorModeled::ValidationException(
                crate::state::invalid_entity_type(&input.entity_type),
            )
            .into());
        }
```
The set it is checked against, services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:238-248:
```
pub const ENTITY_TYPES: &[&str] = &[
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "ExpressPrivateOfferConfiguration",
    "MachineLearningProduct",
    "Offer",
    "OfferSet",
    "ResaleAuthorization",
    "SaaSProduct",
];
```
and state.rs:3001-3004: `pub fn is_entity_type(value: &str) -> bool { ENTITY_TYPES.contains(&value) }`. The constant's own doc (state.rs:225-237) derives the set from `EntitySummary` / `EntityTypeFilters` member names and never mentions the `ListEntitiesRequest$EntityType` doc list.

Test coverage: tests/all/request_validation.rs:541-599 (`an_entity_type_outside_the_catalogs_own_set_is_refused`) pins the nine-entry set as accepted and refuses only invented words (`Frobnicator`, `amiproduct`, `AmiProducts`, `Ami1Product`, a 256-char value) — none of the six documented values above, so nothing in the crate justifies refusing them.

## Steps to Reproduce

1. POST /ListEntities with body {"Catalog": "AWSMarketplace", "EntityType": "Seller"}
2. Observe HTTP 422 ValidationException with Message "Requested entity type 'Seller' is invalid."
3. Expected per `ListEntitiesRequest$EntityType`'s documented valid values: a 200 with `EntitySummaryList` (empty while the catalog holds no Seller entity)
4. Repeat with EntityType = ProcurementPolicy, Experience, Audience, BrandingSettings, Solution — each is refused the same way
