Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the LakeFormation emulator

_Filed: 2026-10-21T21:08:54Z_
_Operation: service-com-amazonaws-lakeformation-awslakeformation | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate `service-com-amazonaws-lakeformation-awslakeformation` has 69 operations and every single one is an unmodified scaffold answering HTTP 501 `NotImplemented` while holding zero state. Nothing about LakeFormation works today.

This issue scopes the FIRST, SMALL slice: the LF-tag lifecycle, which is LakeFormation's primary tag-based-access-control resource and the only self-contained create -> read -> list -> update -> delete loop in the service that does not depend on Glue catalog objects.

Implement these 5 operations, in this order, all `restJson1` POST routes (SDK id `LakeFormation`, signing name `lakeformation`):

1. **CreateLFTag** — `POST /CreateLFTag`. Input: `CatalogId` (optional, len 1..255; default to the caller's account id when absent), `TagKey` (required, len 1..128, pattern `^([\p{L}\p{Z}\p{N}_.:\/=+\-@%]*)$`), `TagValues` (required, list<string>, len 1..50). Output has no members. Must INSERT a tag into state keyed by (CatalogId, TagKey). Creating a TagKey that already exists in that catalog must fail — model `InvalidInputException` (HTTP 400, "Tag key already exists") rather than silently overwriting.
2. **GetLFTag** — `POST /GetLFTag`. Input: `CatalogId` (optional), `TagKey` (required). Output: `CatalogId`, `TagKey`, `TagValues` — all three read back out of the stored tag, with `TagValues` exactly the values persisted by CreateLFTag/UpdateLFTag (not echoed from the request, which does not even carry them). An unknown TagKey must return `EntityNotFoundException` (HTTP 400), not an empty 200.
3. **ListLFTags** — `POST /ListLFTags`. Input: `CatalogId`, `ResourceShareType` (`ALL`|`FOREIGN`), `MaxResults` (1..1000), `NextToken`. Output: `LFTags` (list<LFTagPair> of `{CatalogId, TagKey, TagValues}`) + `NextToken`. Must enumerate the tags actually stored for the catalog, honour `MaxResults` as a page bound, and return an opaque `NextToken` that resumes exactly where the previous page stopped (and is absent on the last page). `@paginated(inputToken=NextToken, outputToken=NextToken, items=LFTags)` is declared in the model, so a paginating SDK client must terminate.
4. **UpdateLFTag** — `POST /UpdateLFTag`. Input: `CatalogId`, `TagKey` (required), `TagValuesToAdd` (list, len 1..50), `TagValuesToDelete` (list, len 1..50). Output has no members. Must MUTATE the stored value set: add the new values (de-duplicated, existing values preserved), remove the deleted ones. Deleting every value of the tag, or deleting/adding nothing at all, is an `InvalidInputException` (HTTP 400); an unknown TagKey is `EntityNotFoundException`. A subsequent GetLFTag must show the merged set.
5. **DeleteLFTag** — `POST /DeleteLFTag`. Input: `CatalogId`, `TagKey` (required). Output has no members. Must REMOVE the tag from state; a following GetLFTag must return `EntityNotFoundException` (HTTP 400) and the tag must no longer appear in ListLFTags. Deleting a TagKey that does not exist is `EntityNotFoundException`.

Hard requirement: every one of these five must compute its response from persisted state held in the single shared engine store reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the request back, no generated identifier that is never stored. A create followed by a get/list must return what was created; a delete must be observable by the next caller.

## State Model

Add to `ComAmazonawsLakeformationAwslakeformationState` (src/state.rs):

```
struct LakeFormationState {
    // catalog id -> (tag key -> LfTag). Catalog id defaults to the caller account id.
    lf_tags: BTreeMap<String, BTreeMap<String, LfTag>>,
}

struct LfTag {
    catalog_id: String,   // echoed back by GetLFTag / ListLFTags
    tag_key: String,      // the stored key, 1..128 chars
    tag_values: Vec<String>, // ordered, de-duplicated, 1..50 entries; never empty
}
```

Pagination: derive `NextToken` from the last-returned `tag_key` within the catalog's ordered map (a `BTreeMap` gives a stable order, so the token can be the exclusive-start key, base64-wrapped) — no separate cursor table needed, and a stale/garbage token is `InvalidInputException`.

Out of scope for this slice (stays 501, to be filed as individual gap issues later): GrantPermissions / ListPermissions / RevokePermissions / BatchGrantPermissions / BatchRevokePermissions, RegisterResource / DescribeResource / ListResources / UpdateResource / DeregisterResource, AddLFTagsToResource / RemoveLFTagsFromResource / GetResourceLFTags, SearchDatabasesByLFTags / SearchTablesByLFTags, the LF-tag expression, data-cells-filter, RLS-policy, transaction, query-planning, identity-center and temporary-credential families.

## Evidence

1) `services/service-com-amazonaws-lakeformation-awslakeformation/OPERATIONS.md` lists 69 operations rendered from `smithy-model/com-amazonaws-lakeformation-awslakeformation.json`, including the five chosen here with the exact routes, member names, constraints and errors quoted above: CreateLFTag (line 96), DeleteLFTag (138), GetLFTag (237), ListLFTags (354, with the `@paginated(inputToken=NextToken, outputToken=NextToken, items=LFTags)` trait), UpdateLFTag (482). The model declares `EntityNotFoundException` (400) and `InvalidInputException` (400) on all five, which is the grounding for the error behaviour above.
2) `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/` matches every operation file in the crate (69/69).
3) `src/ops/create_lf_tag.rs:164` is representative: `handle` binds `let _ = (&input, ctx, &self.engine);` and returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateLFTag"))` — no state access at all.
4) No test exercises any LakeFormation operation; `tests/` contains only the generated `state_roundtrip.rs` example.

## Steps to Reproduce

1. Point an AWS SDK/CLI client for `lakeformation` at the emulator endpoint.
2. `CreateLFTag` with TagKey=`environment`, TagValues=[`dev`,`prod`] -> expect 200 with an empty body (today: 501 NotImplemented).
3. `GetLFTag` with TagKey=`environment` -> expect 200 with CatalogId set, TagKey=`environment`, TagValues containing exactly `dev` and `prod` read back from state.
4. `CreateLFTag` again with TagKey=`environment` -> expect 400 InvalidInputException (tag key already exists), not a second silent overwrite.
5. `ListLFTags` with MaxResults=1 after creating a second tag `team` -> expect one LFTagPair plus a NextToken; call again with that NextToken -> expect the remaining tag and no NextToken.
6. `UpdateLFTag` with TagKey=`environment`, TagValuesToAdd=[`staging`], TagValuesToDelete=[`dev`] -> expect 200; then `GetLFTag` -> TagValues is exactly [`prod`,`staging`].
7. `UpdateLFTag` with TagKey=`environment`, TagValuesToDelete=[`prod`,`staging`] (all remaining values) -> expect 400 InvalidInputException.
8. `DeleteLFTag` with TagKey=`environment` -> expect 200; then `GetLFTag` for it -> expect 400 EntityNotFoundException, and `ListLFTags` no longer includes it.
9. `GetLFTag` / `DeleteLFTag` / `UpdateLFTag` with TagKey=`never-created` -> expect 400 EntityNotFoundException.
