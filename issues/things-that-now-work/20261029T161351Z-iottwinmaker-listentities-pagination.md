Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 15a7913299c0ebfe5f22c92412d933bd37150a6d

# A ListEntities nextToken is scoped to the workspace but not to the `filters` that selected its page, so resuming it under a different filter silently drops matching entities

_Filed: 2026-10-29T16:13:51Z_
_Operation: ListEntities | Class: pagination_

> _Note: this issue was identified against the code base at commit `15a7913299c0ebfe5f22c92412d933bd37150a6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListEntities pages a FILTERED set: `matches_filter` is applied before the resume key is skipped past, so which entities are in the sequence a token names a position in is decided by the request's `filters` entry (`parentEntityId`, `componentTypeId`, `externalId`). The token, however, is bound only to the workspace. A token minted by one filtered listing is therefore accepted verbatim by a listing with a different filter (or none), and because the resume is a `skip_while` over the *newly* filtered sequence, every entity that satisfies the new filter but sorts BEFORE the token's entity id is silently dropped from the walk — the caller is answered 200 with a page that is missing rows and no indication that anything was skipped.

This crate already states the opposite rule and already implements it for its other query-shaped page: `GetPropertyValue` folds every request member that decides which rows there are into the token's scope, and `crate::state::property_filters_token_scope` spells out why. ListEntities is the same situation with the filter left out of the scope.

## Evidence

The token carries only the workspace — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_entities.rs:339-342:
```
fn encode_next_token(workspace_id: &str, entity_id: &str) -> String {
    crate::state::encode_page_token(NEXT_TOKEN_PREFIX, &[workspace_id], entity_id)
}
```
and the handler claims that is enough — list_entities.rs:218-222:
```
// A `nextToken` carries the id the page resumes AT (inclusive), not an
// offset, so a page boundary survives an entity being created or
// deleted between two pages. It is bound to the workspace it was minted
// for, so a token from another listing is refused rather than silently
// resumed.
```
The filter is applied BEFORE the skip, which is what loses the rows — list_entities.rs:251-259:
```
let mut matching = workspace
    .entities
    .values()
    .filter(|entity| matches_filter(workspace, entity, filter))
    .skip_while(|entity| {
        resume_at
            .as_deref()
            .is_some_and(|from| entity.entity_id.as_str() < from)
    });
```
The sibling paged query in the SAME crate binds exactly these members — src/ops/get_property_value.rs:1149-1153:
```
/// What a `nextToken` is bound to besides its workspace: the members that
/// decide WHICH properties of which components the page is of, and — for a
/// TABULAR page — the conditions that decide which rows there are and in what
/// order ([`conditions_scope`]), so a token is only resumed by the query that
/// minted it.
```
and does so in code — get_property_value.rs:1163-1176 renders `tabular_conditions` into the scope via `conditions_scope(input.tabular_conditions.as_ref())`. The shared helper states the rule generally — src/state.rs:4501-4506:
```
/// An ordinal only means something inside the ONE sequence it was counted in, so
/// a listing that resumes at one binds its token to every request member that
/// decides that sequence — and a filter decides it, since a value no filter
/// selects is not in it.
```

Test coverage: tests/all/workspace_entity_lifecycle.rs:708-724 walks ListEntities tokens and :752-761 refuses a token minted for another workspace, but no case resumes a token under a different `filters`; tests/all/entity_component_type_filter.rs and tests/all/entity_external_id_filter.rs exercise the filters and mention neither `maxResults` nor `nextToken` (grep for `nextToken|maxResults` in entity_component_type_filter.rs: "No matches."). Nothing pins the current behaviour.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateEntity `a` with parentEntityId `$ROOT`, then CreateEntity `b` and `c` both with parentEntityId `a`.
3. ListEntities on `w` with `{"maxResults": 1, "filters": [{"parentEntityId": "a"}]}` — the page reports `b` and mints a nextToken whose resume key is `c`.
4. ListEntities on `w` again with `{"maxResults": 10, "nextToken": <that token>}` and NO filters.
5. Observed: 200 with only `c`; `a` and `b` — both members of the unfiltered listing and both sorting before `c` — are silently skipped.
6. Expected (the rule get_property_value.rs:1149-1153 and state.rs:4501-4506 state): the token is bound to the members that decide the sequence, so a request whose `filters` differ from the ones the token was minted under is refused with the modelled ValidationException (crate::state::invalid_next_token_message) rather than resumed.
