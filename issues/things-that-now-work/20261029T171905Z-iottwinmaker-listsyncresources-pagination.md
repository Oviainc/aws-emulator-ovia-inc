Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# A ListSyncResources nextToken is scoped to the workspace and sync source but not to the `filters` that selected its page, so resuming it under a different filter silently drops matching resources

_Filed: 2026-10-29T17:19:05Z_
_Operation: ListSyncResources | Class: pagination_

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListSyncResources pages a FILTERED set. The resume key is applied as a `range(from..)` over `job.sync_resources` and the request's `filters` are applied AFTER that range, so which resources are in the sequence a token names a position in is decided by `filters` (`state`, `resourceType`, `resourceId`, `externalId`). The token itself, however, is bound only to `workspaceId` and `syncSource`.

A token minted by one filtered listing is therefore accepted verbatim by a listing with a different filter (or none at all). Because the resume is a `range` from the token's external id and the newly chosen filter is applied only to what that range yields, every resource that satisfies the NEW filter but sorts BEFORE the token's external id is silently dropped from the walk — the caller is answered 200 with a page that is missing rows and no indication anything was skipped.

This crate already states the opposite rule and already implements it for its other filtered page: `GetPropertyValue` folds every request member that decides which rows there are (including `tabularConditions.propertyFilters`) into the token's scope, and `crate::state::property_filters_token_scope` spells out why a filter decides the sequence. ListSyncResources is the same situation with the filter left out of the scope — and the handler's own doc asserts the token is scoped tightly enough that "a token from another listing — or from this listing taken over another job — is refused rather than silently resumed", which is exactly the guarantee it does not provide across a filter change.

## Evidence

The token carries only the workspace and the sync source — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_sync_resources.rs:513-521:
```
fn encode_next_token(workspace_id: &str, sync_source: &str, external_id: &str) -> String {
    crate::state::encode_page_token(NEXT_TOKEN_PREFIX, &[workspace_id, sync_source], external_id)
}

/// The external id inside a `nextToken` minted over this sync job, or `None`
/// when the token is not one this listing minted for it.
fn decode_next_token(token: &str, workspace_id: &str, sync_source: &str) -> Option<String> {
    crate::state::decode_page_token(NEXT_TOKEN_PREFIX, &[workspace_id, sync_source], token)
}
```
and the handler claims that scope is enough — list_sync_resources.rs:296-301:
```
// A `nextToken` carries the `externalId` the page resumes AT
// (inclusive), not an offset, so a page boundary survives the
// synchronised set changing between two pages. It is bound to the job it
// was minted over, so a token from another listing — or from this
// listing taken over another workspace or sync source — is refused
// rather than silently resumed.
```
The resume happens BEFORE the filter is applied, which is what loses the rows — list_sync_resources.rs:334-339:
```
let mut matching = match &resume_at {
    Some(from) => job.sync_resources.range(from.clone()..),
    None => job.sync_resources.range::<String, _>(..),
}
.map(|(_, resource)| resource)
.filter(|resource| filters.iter().all(|filter| matches_filter(resource, filter)));
```
and the leftover row the token is minted from is itself a FILTERED one — list_sync_resources.rs:361-367 (`matching.next().map(|resource| encode_next_token(...))`), so the token's key is only meaningful inside the filtered sequence.

The sibling paged query in the SAME crate binds exactly these members — src/ops/get_property_value.rs:1149-1153:
```
/// What a `nextToken` is bound to besides its workspace: the members that
/// decide WHICH properties of which components the page is of, and — for a
/// TABULAR page — the conditions that decide which rows there are and in what
/// order ([`conditions_scope`]), so a token is only resumed by the query that
/// minted it.
```
and the shared helper states the rule generally — src/state.rs:4501-4506:
```
/// An ordinal only means something inside the ONE sequence it was counted in, so
/// a listing that resumes at one binds its token to every request member that
/// decides that sequence — and a filter decides it, since a value no filter
/// selects is not in it.
```

Test coverage: tests/all/sync_resource_listing.rs:807-843 (`filteredPaging`) pages a filtered listing but re-sends the SAME `filters` on the resume (line 831: `json!({"maxResults": 1, "filters": filters_body(&in_sync), "nextToken": token})`), and tests/all/sync_resource_listing.rs:1020-1067 refuses tokens minted over another workspace, another sync source, another listing and with an empty resume key — but no case resumes one of this listing's own tokens under a DIFFERENT `filters`. Nothing pins the current behaviour.

## Steps to Reproduce

1. CreateWorkspace `w`, then CreateSyncJob on `w` with syncSource `SITEWISE` so the job holds sync resources (the test harness in tests/all/sync_resource_listing.rs seeds a job with resources in several states).
2. Arrange (or observe in the seeded job) resources whose externalIds sort as `a`, `b`, `c` where `a` is NOT `IN_SYNC` and `b`, `c` ARE.
3. ListSyncResources on `w`/`SITEWISE` with `{"maxResults": 1, "filters": [{"state": "IN_SYNC"}]}` — the page reports `b` and mints a nextToken whose resume key is `c`.
4. ListSyncResources on `w`/`SITEWISE` again with `{"maxResults": 10, "nextToken": <that token>}` and NO filters.
5. Observed: 200 with only `c`; `a` and `b` — both members of the unfiltered listing and both sorting before `c` — are silently skipped, and nothing in the response says so.
6. Expected (the rule get_property_value.rs:1149-1153 and state.rs:4501-4506 state): the token is bound to the members that decide the sequence, so a request whose `filters` differ from the ones the token was minted under is refused with the modelled ValidationException (crate::state::invalid_next_token_message) rather than resumed.
