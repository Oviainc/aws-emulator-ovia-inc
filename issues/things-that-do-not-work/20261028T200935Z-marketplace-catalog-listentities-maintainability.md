Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# Derive the per-entity-type sort attributes from the generated *_SORT_BY_VALUES constants instead of hand-maintaining a second copy

_Filed: 2026-10-28T20:09:35Z_
_Operation: ListEntities | Class: maintainability_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Which attributes a `ListEntities` of one entity type may sort on is spelled out twice inside this crate. `crate::state::entity_type_sort_fields` hand-writes, per type, that type's sort fields with the two common ones dropped, and `entity_sort_attributes` adds `ENTITY_SORT_BY` back on — reconstructing exactly the set the generated `*_SORT_BY_VALUES` constant in `src/ops/types/shared_types.rs` already carries for that type's `*SortBy` enum. The two copies agree today for all nine shaped types, so nothing in any response changes; they are simply two places that have to be edited together, and the generated one is re-rendered from the model while the hand-written one is not, so a sort field added to (or removed from) a type's `*SortBy` enum silently moves only one of them.

## Evidence

Copy 1 — hand-written, services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:1134-1172:
```
pub fn entity_type_sort_fields(entity_type: &str) -> &'static [&'static str] {
    match entity_type {
        "AmiProduct" | "DataProduct" | "MachineLearningProduct" => &["ProductTitle", "Visibility"],
        "ContainerProduct" => &["CompatibleAWSServices", "ProductTitle", "Visibility"],
        "SaaSProduct" => &["DeliveryOptionTypes", "ProductTitle", "Visibility"],
        "Offer" => &[ "AvailabilityEndDate", "BuyerAccounts", "CreatedBySource", "Name", "OfferSetId", "ProductId", "ReleaseDate", "ResaleAuthorizationId", "State", "TargetAgreementId", "TargetAgreementIntent", "Targeting" ],
        "OfferSet" => &["Name", "ReleaseDate", "SolutionId", "State"],
        ...
```
recombined at src/state.rs:1177-1181:
```
pub fn entity_sort_attributes(entity_type: &str) -> Vec<&'static str> {
    let mut attributes = ENTITY_SORT_BY.to_vec();
    attributes.extend_from_slice(entity_type_sort_fields(entity_type));
    attributes
}
```

Copy 2 — generated from the model, src/ops/types/shared_types.rs:126-127, 142-143, 198-199, 190-191:
```
/// The values `AmiProductSortBy` accepts on the wire.
pub const AMI_PRODUCT_SORT_BY_VALUES: &[&str] = &["EntityId", "LastModifiedDate", "ProductTitle", "Visibility"];
/// The values `ContainerProductSortBy` accepts on the wire.
pub const CONTAINER_PRODUCT_SORT_BY_VALUES: &[&str] = &["CompatibleAWSServices", "EntityId", "LastModifiedDate", "ProductTitle", "Visibility"];
/// The values `OfferSortBy` accepts on the wire.
pub const OFFER_SORT_BY_VALUES: &[&str] = &["AvailabilityEndDate", "BuyerAccounts", "CreatedBySource", "EntityId", "LastModifiedDate", "Name", "OfferSetId", "ProductId", "ReleaseDate", "ResaleAuthorizationId", "State", "TargetAgreementId", "TargetAgreementIntent", "Targeting"];
/// The values `OfferSetSortBy` accepts on the wire.
pub const OFFER_SET_SORT_BY_VALUES: &[&str] = &["EntityId", "LastModifiedDate", "Name", "ReleaseDate", "SolutionId", "State"];
```
`ENTITY_SORT_BY` (src/state.rs:117) is `&["LastModifiedDate", "EntityId"]`, so `entity_sort_attributes(t)` and the generated `*_SORT_BY_VALUES` for `t` are the same set for every one of the nine shaped types (AmiProduct, ContainerProduct, DataProduct, MachineLearningProduct, SaaSProduct, Offer, OfferSet, ResaleAuthorization, ExpressPrivateOfferConfiguration).

The consumer is the single `Sort$SortBy` check in src/ops/list_entities.rs:290-296:
```
        let sort_attributes = crate::state::entity_sort_attributes(&input.entity_type);
        if !sort_attributes.contains(&sort_by.as_str()) {
            return Err(ListEntitiesErrorModeled::ValidationException(
                crate::state::invalid_sort_by(&sort_by, &sort_attributes),
```
Existing coverage that must stay green and unchanged: tests/all/entity_type_projections.rs (per-type sort rows, e.g. `json!({"OfferSort": {"SortBy": "State", "SortOrder": "ASCENDING"}})` at :737) and tests/all/sort_order_direction.rs. Note the refusal message quotes the attribute list, so the ORDER of the combined list must be preserved for `invalid_sort_by` to render identically.

## Steps to Reproduce

1. Open src/state.rs:1134 and read `entity_type_sort_fields`; note the per-type attribute lists.
2. Open src/ops/types/shared_types.rs:127 and read the generated `*_SORT_BY_VALUES` constants; note that each is the same set as `ENTITY_SORT_BY` plus the corresponding arm of `entity_type_sort_fields`.
3. Add a value to any `*SortBy` enum in the Smithy model and re-scaffold: `shared_types.rs` picks it up, `entity_type_sort_fields` does not, so `ListEntities` refuses a `Sort$SortBy` the model now declares.
