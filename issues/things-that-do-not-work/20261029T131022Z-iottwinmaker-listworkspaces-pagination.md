Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# A nextToken carries no Region, so a token minted at one Region's endpoint is resumed at another's and silently skips that Region's earlier rows

_Filed: 2026-10-29T13:10:22Z_
_Operation: ListWorkspaces | Class: pagination_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate mints its `nextToken` through `crate::state::encode_page_token(envelope, scope, key)`, and NONE of the scopes carries the Region the page was taken in — except `ListTagsForResource`, whose scope happens to be an ARN (which embeds the Region).

That contradicts the crate's own two statements about what the scope is for and what a Region means here. `encode_page_token`'s doc says the scope is "the parent resource whose page it is, which makes a token minted for another resource, by another listing, or by hand recognisably not this listing's", and `state.rs`'s module doc says "a resource created through one Region's endpoint is invisible through another's and the same id may name an independent resource in each". So the scope `&[workspace_id]` that `ListScenes`, `ListEntities`, `ListComponentTypes`, `ListSyncJobs` (and `&[workspace_id, sync_source]` for `ListSyncResources`, `&[source_type, destination_type]` for `ListMetadataTransferJobs`, `&[workspace_id, statement]` for `ExecuteQuery`) bind a token to does NOT identify the resource the page was taken over: it identifies a *Region-independent id*, and the crate itself says that id names an independent resource in each Region. `ListWorkspaces` is the plainest case — its scope is literally empty (`&[]`), so its token is bound to nothing but the listing kind.

The effect is the thing a key-based cursor exists to prevent: rows are silently dropped. `ListWorkspaces` resumes the walk at a `workspaceId` with `state.workspaces.range(from.clone()..)`, so a token minted in `us-east-1` naming `b` is accepted at the `eu-west-1` endpoint and positions that Region's independent store at `b`, skipping every workspace of `eu-west-1` that sorts before it — HTTP 200, no refusal, and no indication that the page was cut. Each handler claims the opposite in a comment two lines above the decode ("a token from another listing — or from this listing taken over another workspace — is refused rather than silently resumed").

This is not the Region-scoping gap the FIXED issue `20261024T075111Z-iottwinmaker-listworkspaces-wrong-behavior` closed (the stores ARE regional now); it is the one place the regionalisation did not reach — the cursor.

## Evidence

WHAT THE CRATE SAYS A REGION MEANS — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:11-14:
```
//! Region to its store. A request is served out of the store of the Region its
//! own credential scope names ([`request_region`]), so a resource created
//! through one Region's endpoint is invisible through another's and the same id
//! may name an independent resource in each.
```

WHAT THE CRATE SAYS THE TOKEN SCOPE IS FOR — src/state.rs:4334-4353:
```
/// The opaque `nextToken` every paginated operation of this service mints.
///
/// One shape, shared by all of them so a token means the same thing wherever it
/// is read: hex (so the token is opaque to a caller and safe in a JSON body)
/// over an `envelope` naming the LISTING it belongs to, the resume `key` it
/// resumes at, and the `scope` the listing was taken in — the parent resource
/// whose page it is, which makes a token minted for another resource, by another
/// listing, or by hand recognisably not this listing's.
...
pub fn encode_page_token(envelope: &str, scope: &[&str], key: &str) -> String {
```

THE SCOPE THAT CARRIES NO REGION — src/ops/list_workspaces.rs:252-267:
```
/// The envelope a `nextToken` of this listing carries, so a token minted
/// elsewhere — by another listing of this service, or by hand — is recognisably
/// not one of ours. ... this listing is
/// Region-wide, so unlike `ListEntities`' it is scoped to no parent resource.
const NEXT_TOKEN_PREFIX: &str = "iottwinmaker:workspaces:1:";

/// The opaque `nextToken` for the page of workspaces starting at `workspace_id`.
fn encode_next_token(workspace_id: &str) -> String {
    crate::state::encode_page_token(NEXT_TOKEN_PREFIX, &[], workspace_id)
}
...
fn decode_next_token(token: &str) -> Option<String> {
    crate::state::decode_page_token(NEXT_TOKEN_PREFIX, &[], token)
}
```
and the walk it positions, src/ops/list_workspaces.rs:205-212:
```
        // `workspaces` is keyed by `workspaceId`, so iterating the map IS id
        // order and resuming is a `range` from the token's key: the page starts
        // at the first id at or past it without walking what came before.
        let mut remaining = match &resume_at {
            Some(from) => state.workspaces.range(from.clone()..),
            None => state.workspaces.range::<String, _>(..),
        }
```
while the store it walks is this Region's alone, src/ops/list_workspaces.rs:167-174:
```
        let region = crate::state::request_region(ctx.parts());
        ...
        let state = state.region(&region);
```

THE CLAIM EACH HANDLER MAKES ABOUT ITS TOKEN, which the Region case breaks — src/ops/list_scenes.rs:216-220:
```
        // A `nextToken` carries the `sceneId` the page resumes AT (inclusive),
        // not an offset, so a page boundary survives a scene being created or
        // deleted between two pages. It is bound to the workspace it was minted
        // for, so a token from another listing — or from this listing taken over
        // another workspace — is refused rather than silently resumed.
```
with the scope that cannot tell two Regions' workspaces apart, src/ops/list_scenes.rs:309-311:
```
fn encode_next_token(workspace_id: &str, scene_id: &str) -> String {
    crate::state::encode_page_token(NEXT_TOKEN_PREFIX, &[workspace_id], scene_id)
}
```
(the same shape at list_entities.rs:340-342, list_component_types.rs:403-405, list_sync_jobs.rs:329-331, list_sync_resources.rs:513-515, list_metadata_transfer_jobs.rs:500-510, list_properties.rs:456-462, execute_query.rs:2077-2086.)

THE ONE SIBLING THAT IS REGION-BOUND, which shows the scope CAN carry it — src/ops/list_tags_for_resource.rs:254-256:
```
fn encode_next_token(resource_arn: &str, tag_key: &str) -> String {
    crate::state::encode_page_token(NEXT_TOKEN_PREFIX, &[resource_arn], tag_key)
}
```
because an ARN embeds the Region — src/state.rs:4229-4235:
```
pub fn workspace_arn(region: &str, workspace_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("{WORKSPACE_ARN_RESOURCE_PREFIX}{workspace_id}"),
    )
}
```

TEST COVERAGE: no row exercises a token across Regions. tests/all/region_scoping.rs:9-13 pins the Region rule ("a resource exists only in the Region whose endpoint created it") over every resource kind but never sends a `nextToken`. tests/all/next_token_refusal.rs:6-13 pins the refusal for "every paginated operation this service serves" and even for "a REAL token another listing minted", but every call it makes goes through `Emulator::call`, which signs for ONE Region — tests/all/harness.rs:135-136:
```
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Resp {
        self.call_signed_for(Some(REGION), method, path, body).await
```
so "a token this listing did not mint" is never tested in its cross-Region form.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="alpha") and CreateWorkspace(workspaceId="beta"), both signed for us-east-1.
2. ListWorkspaces(maxResults=1) signed for us-east-1 -> 200 with workspaceSummaries=[alpha] and a nextToken (the token names "beta").
3. CreateWorkspace(workspaceId="alpha") and CreateWorkspace(workspaceId="beta"), both signed for eu-west-1 (the same ids are free there — tests/all/region_scoping.rs:167-178 pins exactly that).
4. ListWorkspaces(maxResults=1, nextToken=<the us-east-1 token>) signed for eu-west-1 -> 200 with workspaceSummaries=[beta]: eu-west-1's "alpha" is silently skipped, although no listing of eu-west-1's store ever minted that token and the handler's own comment says such a token is "refused rather than silently resumed".
5. Repeat with any workspace-scoped listing, e.g. CreateScene in both Regions' "alpha" workspace, ListScenes(maxResults=1) in us-east-1, then resume that token in eu-west-1: the token's scope is only the workspace id, which state.rs:11-14 says names an independent workspace per Region.
