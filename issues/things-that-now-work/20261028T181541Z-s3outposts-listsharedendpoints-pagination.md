Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: afab077872ee6c32a9626f154d3b33fea96b8a18

# A NextToken whose Outpost half disagrees with the request's OutpostId is served as an arbitrary (often empty) page instead of being refused

_Filed: 2026-10-28T18:15:41Z_
_Operation: ListSharedEndpoints | Class: pagination_

> _Note: this issue was identified against the code base at commit `afab077872ee6c32a9626f154d3b33fea96b8a18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSharedEndpoints` is scoped by a **required** `OutpostId`, and its cursor is the `(OutpostId, EndpointId)` pair the next page starts at. The handler decodes the cursor and resolves the Outpost independently and never compares the two, so a cursor whose Outpost half names a DIFFERENT Outpost than the request's `OutpostId` is accepted and fed straight into the shared walk.

The walk's only use of the cursor is `if key < from { continue; }` over `store.endpoints` (keyed `(OutpostId, EndpointId)`), while `include` restricts the walk to `endpoint.outpost_id == input.outpost_id`. The two filters therefore interact:

* when the cursor's Outpost sorts AFTER the requested one, EVERY key of the requested Outpost is `< from`, so the handler answers HTTP 200 with `Endpoints: []` and no `NextToken` — a confident "this Outpost has no endpoints" for an Outpost that has them;
* when it sorts BEFORE, the cursor is a no-op and the handler re-serves the FIRST page of the requested Outpost, which is neither a resumption nor an error.

That is exactly the outcome `state::list_cursor`'s own doc says never happens ("answered rather than served as an arbitrary page"), and it turns a malformed resume into a success that misreports persisted state. The refusal path already exists in this crate — `list_cursor` takes the `decode` closure and mints the `ValidationException` — so nothing new has to be invented; `decode_endpoint_cursor` simply validates the token's FORM and nothing about the scope it is being resumed in.

The existing pagination tests never exercise this seam: every resume case uses a cursor from the SAME listing, and the "never issued" rows only vary the token's FORM (the other listing's cursor form, empty halves, wrong widths).

## Evidence

The rule the crate states for every cursor — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:979-983:

    /// The key a listing resumes its walk at for `next_token`, as `decode` reads one
    /// out of a cursor: `None` for a request that carries no cursor, and the refusal
    /// for a token this service never issued — which is answered rather than served
    /// as an arbitrary page.

What the endpoint cursor actually validates — state.rs:1103-1109 (the FORM only; nothing about the listing's scope):

    /// The `(OutpostId, EndpointId)` pair an endpoint listing's cursor names, or
    /// `None` when the token is not one this service issued.
    pub fn decode_endpoint_cursor(token: &str) -> Option<(String, String)> {
        let (outpost_id, endpoint_id) = token.split_once('/')?;
        (is_outpost_id(outpost_id) && is_endpoint_id(endpoint_id))
            .then(|| (outpost_id.to_string(), endpoint_id.to_string()))
    }

The handler decodes the cursor and resolves the Outpost without ever comparing them — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_shared_endpoints.rs:157-190:

    // The cursor is the (Outpost, endpoint) pair the next page starts at. It is
    // decoded before the Outpost is resolved, so a token this service never
    // issued is refused whatever the request named.
    let resume = crate::state::endpoint_cursor(input.next_token.as_deref(), Self::NAME)
        .map_err(ListSharedEndpointsErrorModeled::ValidationException)?;
    ...
    crate::state::resolve_outpost(&inventory, &region, &input.outpost_id)
        .map_err(ListSharedEndpointsErrorModeled::ResourceNotFoundException)?;
    ...
        |endpoint| endpoint.outpost_id == input.outpost_id,

The two filters that collide in the shared walk — state.rs:1030-1044:

    for (key, record) in records {
        if !include(record) {
            continue;
        }
        // Resume at the first key at or after the cursor, so a page is still
        // served when the record the cursor named was deleted meanwhile.
        if resume.as_ref().is_some_and(|from| key < from) {
            continue;
        }

with `endpoints: BTreeMap<(String, String), EndpointRecord>` keyed "by `(OutpostId, EndpointId)` ... which is also the stable order the endpoint listings page over" (state.rs:469-479) — so the tuple comparison makes the cursor's Outpost half dominate over records of a different Outpost.

Test coverage of the seam: NONE. services/service-com-amazonaws-s3outposts-s3outposts/tests/all/one_listing_rule.rs:140-153 lists the only refused tokens, all FORM failures ("no separator at all", "the other listing's Outpost cursor", "an empty Outpost id", "an empty endpoint id", "an Outpost id of no published form", "an endpoint id one character short"), and one_listing_rule.rs:296-312 only ever resumes with `listing.stored_keys(&engine)` — a cursor of the SAME listing: `let answer = listing.page(&router, Some(100), Some(cursor)).await;`. tests/all/shared_endpoint_listing.rs:242-297 likewise pages each Outpost with its own tokens only.

## Steps to Reproduce

1. Stand up the emulator with two Outposts in the request's Region whose ids sort apart, e.g. B = "op-00000000000000000" and A = "op-fffffffffffffffff" (the seam tests/all/endpoint_lifecycle.rs:66-82 `emulator_holding` uses, or S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-00000000000000000:1024,op-fffffffffffffffff:1024").
2. Create two endpoints on A and two on B via POST /S3Outposts/CreateEndpoint (each with its own SubnetId/SecurityGroupId).
3. GET /S3Outposts/ListSharedEndpoints?outpostId=op-fffffffffffffffff&maxResults=1 and read the NextToken it mints: "op-fffffffffffffffff/<19-char endpoint id>" — a token this service DID issue.
4. Replay that token against the other Outpost: GET /S3Outposts/ListSharedEndpoints?outpostId=op-00000000000000000&maxResults=100&nextToken=op-fffffffffffffffff/<19-char endpoint id>
5. Observe HTTP 200 with `{"Endpoints": []}` and no NextToken — the handler reports that Outpost B has no endpoints, although GET /S3Outposts/ListSharedEndpoints?outpostId=op-00000000000000000&maxResults=100 (no token) reports its two endpoints. Expected per state.rs:979-983: the cursor is not one this listing's walk could have minted for this scope, so it is refused rather than "served as an arbitrary page".
6. Mirror case: replay a token minted for Outpost B against outpostId=op-fffffffffffffffff — the cursor is silently ignored and the FIRST page of A is served again, so a client paging with it never advances.
