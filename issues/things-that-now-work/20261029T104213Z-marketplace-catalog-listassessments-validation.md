Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6bb9010ea97ecc711aeaca7897e143f375b8cacc

# A FrameworkId naming no framework of this catalog silently answers an empty page instead of being refused

_Filed: 2026-10-29T10:42:13Z_
_Operation: ListAssessments | Class: validation_

> _Note: this issue was identified against the code base at commit `6bb9010ea97ecc711aeaca7897e143f375b8cacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAssessments` narrows its page by `FrameworkId` ("The unique identifier of a framework. When specified, only assessments performed against this framework are returned. For example, `AMISecurity`"). The handler passes the submitted value straight into `AssessmentRecord::matches_framework_id` and never judges it against the frameworks this catalog has, so a mistyped or invented framework id (`AMISecurty`, `FooSecurity`) selects nothing and is answered 200 with an empty `AssessmentSummaryList` — indistinguishable, to the caller, from "this framework assessed nothing".

That is the opposite of the rule this crate states and applies everywhere else for a member whose value set the model publishes, and the closed set is already declared in this very module: `ASSESSMENT_FRAMEWORKS`, documented as "The frameworks this catalog assesses against, named by the `FrameworkSummary` and `FrameworkFilters` union members the model declares". The refusal helper is there too (`invalid_choice`), which is what `ListChangeSets` uses for its `Status` filter values and `ListEntities` for its `EntityType` — both narrowing members of the same kind on the sibling list operations.

## Evidence

Model (smithy_query query="doc", shape="ListAssessmentsRequest", member="FrameworkId"): "The unique identifier of a framework. When specified, only assessments performed against this framework are returned. For example, <code>AMISecurity</code>." The frameworks themselves are the two the model's unions name (`AMISecurityFilters`/`AMISecuritySummary`, `ContainerSecurityFilters`/`ContainerSecuritySummary`).

The rule this crate states for exactly this situation — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:135-139`:
```
/// A filter selects on an attribute, so when the model enumerates that
/// attribute's values, a filter value outside that enum names no value the
/// attribute can hold — refused, as every other value of this service naming
/// nothing the catalog has is refused, rather than silently selecting the empty
/// page it would match.
```

The closed set and the refusal helper the crate already owns — `src/state.rs:504-511`:
```
/// The frameworks this catalog assesses against, named by the `FrameworkSummary`
/// and `FrameworkFilters` union members the model declares (`AMISecuritySummary`
/// / `AMISecurityFilters` -> `AMISecurity`, `ContainerSecuritySummary` /
/// `ContainerSecurityFilters` -> `ContainerSecurity`); …
pub const ASSESSMENT_FRAMEWORKS: &[&str] =
    &[FRAMEWORK_AMI_SECURITY, FRAMEWORK_CONTAINER_SECURITY];
```
and `src/state.rs:3522-3526` (`invalid_choice`).

What the handler does instead — `src/ops/list_assessments.rs:226-243`:
```
        let mut matched: Vec<&crate::state::AssessmentRecord> = store
            .assessments
            .values()
            .filter(|record| {
                input
                    .framework_id
                    .as_deref()
                    .is_none_or(|framework_id| record.matches_framework_id(framework_id))
```
with `src/state.rs:1337-1342` comparing only against what the records happen to hold:
```
    pub fn matches_framework_id(&self, framework_id: &str) -> bool {
        if self.framework_id == framework_id {
            return true;
        }
        self.framework == framework_id
    }
```

The sibling narrowing members ARE judged against their published sets:
- `src/ops/list_entities.rs:193-198`: `if !crate::state::is_entity_type(&input.entity_type) { return Err(ListEntitiesErrorModeled::ValidationException(crate::state::invalid_entity_type(&input.entity_type)) …` — with the comment "a letters-only value naming no type at all names nothing to list and is refused before any listing is performed" (`:186-192`).
- `src/state.rs:140-145` + `:215-222` (`filter_value_set` / `filter_predicate`): `if let Some((noun, published)) = filter_value_set(&name) { if let Some(outside) = values.iter().find(|value| !published.contains(&value.as_str())) { return Err(invalid_choice(noun, outside, published)); } }` — the check `ListChangeSets` applies to its `Status` filter.

Test coverage: `tests/all/assessments.rs` and `tests/all/filter_validation.rs` exercise `FrameworkId` only with the two real frameworks; no test pins the empty page an unknown framework id answers.

## Steps to Reproduce

1. Create an AmiProduct entity with a delivery option so at least one AMISecurity assessment exists (POST /StartChangeSet with a CreateProduct change whose Details carry DeliveryOptions, then wait for SUCCEEDED).
2. POST /ListAssessments with {"Catalog":"AWSMarketplace","MaxResults":20,"FrameworkId":"AMISecurity"} and observe the assessment is listed.
3. POST /ListAssessments with {"Catalog":"AWSMarketplace","MaxResults":20,"FrameworkId":"AMISecurty"} (one character wrong, still inside the member's `^[\w\-@]+$` pattern).
4. Observe HTTP 200 with an empty AssessmentSummaryList, instead of the 422 ValidationException the crate's own rule (state.rs:135-139) and its sibling lists give a value naming nothing the catalog has.
