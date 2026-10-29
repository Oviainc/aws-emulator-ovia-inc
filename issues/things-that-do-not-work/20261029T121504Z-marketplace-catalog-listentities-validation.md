Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# An EntityId filter value that is not an entity id is accepted and silently answers an empty page

_Filed: 2026-10-29T12:15:04Z_
_Operation: ListEntities | Class: validation_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEntities` narrows its page with `FilterList`, whose only published attribute is `EntityId` (`Filter$ValueList`: "ListEntities - This is a list of unique EntityIds"). The shared filter rule `crate::state::filter_predicate` judges a filter's NAME (must be one of the attributes the list publishes), that it carries at least one value, and — only where the model enumerates a closed set for that attribute (`filter_value_set`, which covers `Status` alone) — that each value is inside that set. Nothing judges an `EntityId` filter value against the FORM an entity id has.

So a `ValueList` entry that no entity id can ever be (`"!!"`, `"_x_"`, the two-character `"ab"`) is accepted, matched against the stored ids by plain string equality (`EntityRecord::matches_filter`), matches nothing, and is answered 200 with an empty `EntitySummaryList` — indistinguishable, to the caller, from "this catalog holds no such entity".

That is the opposite of what this crate does on every other member that names an entity. `crate::state::ENTITY_ID_PATTERN`'s own documentation states the rule: `EntityId` "is this service's statement of which strings are entity ids at all", and "the service enforces the `EntityId` form on those members too" — which `DescribeEntity` and `BatchDescribeEntities` both do, BEFORE any lookup, so a malformed id is a 422 (or a per-entry `ValidationException`) rather than a miss. `StartChangeSet` applies the same predicate inside `is_entity_identifier`. The filter path is the one entity-naming surface that skips `is_entity_id`.

Note this is not the closed-set rule a previous fix added: `filter_value_set`'s own comment correctly says an entity id is bounded by "no published set". The gap is the FORM, which the crate states and enforces separately.

Secondary symptom of the same class and the same shared helper: `filter_predicate`'s doc quotes `Filter$Name` as "the name of the filter, which is unique for each filter that you want to apply", but neither list handler checks that the names in one `FilterList` are distinct. Two filters that both name `EntityId` (or, on `ListChangeSets`, two `Status` filters) are both pushed as predicates and conjoined by `.all(...)`, so a request the quoted rule forbids is accepted and silently intersected into an empty page instead of being refused. The same `ValueList` is also never checked for the uniqueness the model's own sentence states ("a list of unique EntityIds").

`ListChangeSets` reaches both symptoms through the same helper for its own `EntityId` filter; the root cause is one place.

## Evidence

Model, `smithy_query(query="doc", shape="Filter", member="ValueList")`:
```
<p><code>ListEntities</code> - This is a list of unique <code>EntityId</code>s.</p>
...
<li><p><code>EntityId</code> - The supported <code>ValueList</code> is a list of unique <code>EntityId</code>s.</p></li>
```

The rule this crate states for every member that names an entity — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:3473-3484`:
```
/// The form an entity id takes, as the model's own `EntityId` shape states it
/// (`@pattern("^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$")`).
///
/// `EntityId` is the shape every member that NAMES an entity targets —
/// `EntityRequest$EntityId`, the `EntityDetails` / `Errors` map keys of
/// `BatchDescribeEntities`, `ValidationExceptionField$EntityId` — so it is this
/// service's statement of which strings are entity ids at all. A handful of
/// members, `DescribeEntityRequest$EntityId` among them, target the wider
/// `ResourceId` (`^[\w\-]+$`) instead, which admits strings no entity id can be
/// (an underscore, or a single character); the service enforces the `EntityId`
/// form on those members too, and the pattern is reported verbatim in the
/// refusal so a caller can see what was required.
pub const ENTITY_ID_PATTERN: &str = "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$";
```

The sibling single read that DOES apply it — `src/ops/describe_entity.rs:177-188`:
```
        // `DescribeEntityRequest$EntityId` targets the wider `ResourceId`
        // (`^[\w\-]+$`), which admits strings that are not entity ids at all.
        // The id is checked against the form the model's own `EntityId` shape
        // states BEFORE any lookup, so a malformed id is refused as a 422
        // `ValidationException` ...
        if !crate::state::is_entity_id(&input.entity_id) {
            return Err(DescribeEntityErrorModeled::ValidationException(
                crate::state::invalid_entity_id(&input.entity_id),
            )
            .into());
        }
```
and the batch read, per entry — `src/ops/batch_describe_entities.rs:188-199`:
```
            if !crate::state::is_entity_id(&request.entity_id) {
                errors.insert(
                    request.entity_id.clone(),
                    BatchDescribeErrorDetail {
                        error_code: Some(crate::state::BATCH_ERROR_VALIDATION.to_string()),
                        error_message: Some(crate::state::invalid_entity_id_message(
                            &request.entity_id,
                        )),
```

The filter path that skips it — `src/ops/list_entities.rs:264-280`:
```
        let filters = input.filter_list.unwrap_or_default();
        let mut predicates: Vec<(String, Vec<String>)> = Vec::with_capacity(filters.len());
        for filter in filters {
            // The same shared rule `ListChangeSets` judges its filters by, over
            // the attributes this list publishes.
            let (name, values) = match crate::state::filter_predicate(
                Self::NAME,
                crate::state::ENTITY_FILTER_NAMES,
                filter,
            ) {
```
and the helper itself, which judges only the name, emptiness and the closed SET — `src/state.rs:196-223`:
```
    let Some(name) = filter.name else { ... };
    if !filtered_on.contains(&name.as_str()) { ... }
    let values = filter.value_list.unwrap_or_default();
    if values.is_empty() { ... }
    if let Some((noun, published)) = filter_value_set(&name) {
        if let Some(outside) = values.iter().find(|value| !published.contains(&value.as_str())) {
            return Err(invalid_choice(noun, outside, published));
        }
    }
    Ok((name, values))
```
with `filter_value_set` returning `None` for `EntityId` — `src/state.rs:140-145`:
```
pub fn filter_value_set(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    match name {
        CHANGE_SET_STATUS_FILTER_NAME => Some(("status", CHANGE_STATUS_VALUES)),
        _ => None,
    }
}
```
and the value then only ever compared for equality — `src/state.rs:1040-1045`:
```
    pub fn matches_filter(&self, name: &str, values: &[String]) -> bool {
        match name {
            "EntityId" => values.iter().any(|value| value == &self.id),
            _ => false,
        }
    }
```

The uniqueness rule the helper's own doc quotes but no handler applies — `src/state.rs:180-182`:
```
/// `Filter$Name` is "the name of the filter, which is unique for each filter
/// that you want to apply" and names an attribute the list being called filters
/// on, while `Filter$ValueList` is "the list of values" that filter matches ...
```
against `src/ops/list_entities.rs:265-279` (every filter is pushed, duplicates included) and `src/ops/list_entities.rs:379-383` (`predicates.iter().all(...)`), plus the identical loop in `src/ops/list_change_sets.rs:176-220`.

Test coverage: `tests/all/filter_validation.rs:570-591` exercises the `EntityId` filter only with well-formed ids — "the id the seed change set minted" and `"ent-000000000000"` ("an id this store does not hold") — and `tests/all/filter_validation.rs:52-67` only lists the supported filter NAMES, so no test pins the acceptance of a value that is not an entity id, and none pins a repeated filter name.

## Steps to Reproduce

1. Start and apply a change set so the catalog holds one AmiProduct entity: POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"alpha\"}"}]} and wait CHANGE_SET_PREPARING + CHANGE_SET_APPLYING.
2. POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"AmiProduct"} and note the EntityId it reports.
3. GET /DescribeEntity?catalog=AWSMarketplace&entityId=_x_ and observe 422 ValidationException "[Requested entity id '_x_' is invalid. It should match with ^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$.]" — the form check the crate applies to every member naming an entity.
4. POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"AmiProduct","FilterList":[{"Name":"EntityId","ValueList":["_x_"]}]} and observe 200 with an empty EntitySummaryList instead of that same 422 — the value is not an entity id at all.
5. Repeat with "ValueList":["ab"] (two characters, which ENTITY_ID_PATTERN excludes) and with "ValueList":["!!"]: both answer 200 with an empty page.
6. POST /ListChangeSets with {"Catalog":"AWSMarketplace","FilterList":[{"Name":"EntityId","ValueList":["_x_"]}]} — same empty page, through the same helper.
7. Duplicate-name symptom: POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"AmiProduct","FilterList":[{"Name":"EntityId","ValueList":["<the real id>"]},{"Name":"EntityId","ValueList":["ent-000000000000"]}]} and observe 200 with an empty page: both filters are applied and conjoined, although filter_predicate's own doc quotes Filter$Name as "unique for each filter that you want to apply".
