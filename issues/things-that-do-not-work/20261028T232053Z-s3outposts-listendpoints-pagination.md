Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 749239411ed702c76c0bcde46fdec58b18957221

# ListEndpoints serves a NextToken whose Outpost half it could never have minted as an arbitrary (often empty) page instead of refusing it

_Filed: 2026-10-28T23:20:53Z_
_Operation: ListEndpoints | Class: pagination_

> _Note: this issue was identified against the code base at commit `749239411ed702c76c0bcde46fdec58b18957221`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEndpoints` reports "the caller's own [endpoints], across every Outpost it has them on in this Region" (ops/list_endpoints.rs:165-170), and its cursor is the `(OutpostId, EndpointId)` pair the next page starts at. The handler validates only the token's FORM: `decode_endpoint_cursor` returns `Some` for any `<OutpostId>/<EndpointId>` string whose halves match `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$` and `EndpointId`'s `^[a-zA-Z0-9]{19}$`. The decoded pair is then fed straight into the shared, Region-scoped walk.

`store.endpoints` is a `BTreeMap` keyed `(OutpostId, EndpointId)`, so the cursor's Outpost half DOMINATES the tuple comparison, and the walk's only use of the cursor is `if key < from { continue; }`. Meanwhile `endpoint_page`'s own filter admits only endpoints whose parent Outpost is installed in the request's Region (`home_region(endpoint) == Some(region)`). The two filters therefore collide for any token whose Outpost half names an Outpost this listing could never have walked — one installed in ANOTHER Region, or one the inventory does not hold at all:

* when that Outpost id sorts AFTER every Outpost the caller has endpoints on in this Region, EVERY stored key is `< from` and the handler answers HTTP 200 with `Endpoints: []` and no `NextToken` — a confident "this account has no endpoints in this Region" for an account whose untokened listing reports several;
* when it sorts BEFORE, the cursor is a silent no-op and the FIRST page is re-served, so a client paging with it never advances.

That is exactly the outcome this crate's own `state::list_cursor` doc says never happens ("answered rather than served as an arbitrary page"), and the outcome the handler's own comment above the call claims it prevents. It also crosses the Region boundary the crate states for every lookup and listing. The refusal machinery already exists here — `list_cursor` takes the `decode` closure and mints the `ValidationException` — so nothing new has to be invented; the decoder simply knows nothing about the scope it is being resumed in.

This is NOT the deleted-record tolerance the walk deliberately keeps (state.rs:1073-1074, "so a page is still served when the record the cursor named was deleted meanwhile"): the complaint is not about the endpoint half of the cursor, it is that the OUTPOST half names a place this Region's listing has no order over at all.

This is a different operation and a different scope test from the two open sibling issues. The open `ListSharedEndpoints` pagination issue is fixed by comparing the cursor's Outpost half with that operation's REQUIRED `OutpostId` member — `ListEndpoints` has no `OutpostId` member at all (OPERATIONS.md:44), so that fix cannot reach this handler. The open `ListOutpostsWithS3` pagination issue lives in `decode_outpost_cursor`, a different decoder over a different map.

## Evidence

The rule the crate states for every cursor — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:1018-1021:

    /// The key a listing resumes its walk at for `next_token`, as `decode` reads one
    /// out of a cursor: `None` for a request that carries no cursor, and the refusal
    /// for a token this service never issued — which is answered rather than served
    /// as an arbitrary page.

The handler repeats it — src/ops/list_endpoints.rs:153-157:

    // The cursor is the (Outpost, endpoint) pair the next page starts at.
    // A token this service never issued is refused rather than served as an
    // arbitrary page.
    let resume = crate::state::endpoint_cursor(input.next_token.as_deref(), Self::NAME)
        .map_err(ListEndpointsErrorModeled::ValidationException)?;

What the endpoint cursor actually validates — the FORM only, nothing about the Region or inventory it is being resumed in — src/state.rs:1142-1148:

    /// The `(OutpostId, EndpointId)` pair an endpoint listing's cursor names, or
    /// `None` when the token is not one this service issued.
    pub fn decode_endpoint_cursor(token: &str) -> Option<(String, String)> {
        let (outpost_id, endpoint_id) = token.split_once('/')?;
        (is_outpost_id(outpost_id) && is_endpoint_id(endpoint_id))
            .then(|| (outpost_id.to_string(), endpoint_id.to_string()))
    }

The scope this listing serves, per the handler's own comment — src/ops/list_endpoints.rs:165-179:

    // The walk, the page bound, the cursor and the Region a listing serves are
    // the one listing rule this service's listings follow, so they live in
    // `crate::state`; what is this operation's own is which endpoints it
    // reports: the caller's own, across every Outpost it has them on in this
    // Region.
    let page = crate::state::endpoint_page(
        &mut store,
        &inventory,
        now,
        &region,
        input.max_results,
        resume,
        |endpoint| endpoint.owner_account == caller,
    );

The two filters that collide in the shared walk — src/state.rs:1069-1077:

    for (key, record) in records {
        if !include(record) {
            continue;
        }
        // Resume at the first key at or after the cursor, so a page is still
        // served when the record the cursor named was deleted meanwhile.
        if resume.as_ref().is_some_and(|from| key < from) {
            continue;
        }

and src/state.rs:1214-1222, where the Region filter the cursor escapes is applied:

    list_page(
        &store.endpoints,
        max_results,
        resume,
        |endpoint| home_region(endpoint) == Some(region) && include(endpoint),
        encode_endpoint_cursor,

with the key order the cursor indexes into — src/state.rs:478-482:

    /// Every endpoint, keyed by `(OutpostId, EndpointId)` — the pair
    /// `DeleteEndpoint` names — which is also the stable order the endpoint
    /// listings page over. The `OutpostId` half is what puts the endpoint in a
    /// Region: it is the Outpost's, and so the endpoint's
    /// ([`outpost_region`]).

and the Region rule the token escapes — src/state.rs:56-63:

    //! resource here belongs to one Region and is reachable only through it:
    //! [`OutpostRecord::region`] records the Outpost's own (home) Region, an endpoint
    //! takes the Region of the Outpost it is on ([`outpost_region`]),
    //! and every lookup and listing resolves inside the Region the request was signed
    //! for alone ([`request_region`]) — so a resource of another Region is reported by
    //! no listing and resolves to the modelled 404, exactly as if the store held one
    //! set of resources per Region.

Test coverage of this seam: NONE. The only refused tokens are FORM failures — tests/all/endpoint_lifecycle.rs:645-663:

    Listing::OwnEndpoints | Listing::EndpointsOn(_) => vec![
        ("no separator at all", "not-a-cursor"),
        ("only the Outpost id, the other listing's cursor form", "op-0a1b2c3d4e5f60718"),
        ("only the endpoint id", "0000000000000000001"),
        ("an empty Outpost id", "/0000000000000000001"),
        ("an empty endpoint id", "op-0a1b2c3d4e5f60718/"),
        ("neither part of a published form", "aaaa/bbbb"),
        ("an Outpost id of no published form", "op-zzzzzzzzzzzzzzzzz/0000000000000000001"),
        ("an endpoint id one character short", "op-0a1b2c3d4e5f60718/000000000000000000"),
    ],

and the only resume cases replay a cursor of the SAME listing in the SAME Region — tests/all/one_listing_rule.rs:113-120:

    for listing in &listings {
        let expected = listing.stored_keys(&engine);
        for (index, cursor) in expected.iter().enumerate() {
            ...
            let answer = listing.page(&router, Some(100), Some(cursor)).await;

tests/all/endpoint_paging.rs:139-177 (`both_endpoint_listings_refuse_a_token_they_never_issued`) likewise drives only `listing.never_issued()`, i.e. the form table above. So refusing a cursor whose Outpost half is not one this listing's walk could have reached leaves every existing test passing.

## Steps to Reproduce

1. Stand up the emulator with two Outposts whose ids sort apart, in different Regions — e.g. `emulator_holding(&[("op-00000000000000000".into(), "us-east-1"), ("op-fffffffffffffffff".into(), "us-west-2")])` (tests/all/endpoint_lifecycle.rs:71-87), or run with S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-00000000000000000:1024:<account>:us-east-1,op-fffffffffffffffff:1024:<account>:us-west-2".
2. Create an endpoint on each: POST /S3Outposts/CreateEndpoint signed for us-east-1 with OutpostId=op-00000000000000000, and POST /S3Outposts/CreateEndpoint signed for us-west-2 with OutpostId=op-fffffffffffffffff (each with its own SubnetId/SecurityGroupId).
3. GET /S3Outposts/ListEndpoints?maxResults=100 signed for us-east-1 — it reports the us-east-1 endpoint.
4. GET /S3Outposts/ListEndpoints?maxResults=1 signed for us-west-2 and read the (OutpostId, EndpointId) pair it reports: "op-fffffffffffffffff/<19-char endpoint id>" is a cursor only us-west-2's listing could mint. (A hand-crafted "op-fffffffffffffffff/0000000000000000001" naming no Outpost at all behaves identically.)
5. Replay it as this Region's cursor: GET /S3Outposts/ListEndpoints?maxResults=100&nextToken=op-fffffffffffffffff/<19-char endpoint id> signed for us-east-1.
6. Observe HTTP 200 with {"Endpoints": []} and no NextToken — the listing reports that the account has no endpoints in us-east-1, although step 3 reported one. Expected per state.rs:1018-1021 and the handler's own comment at list_endpoints.rs:153-155: a token this listing never issued (and, naming another Region's Outpost, could never issue) is refused with the 400 ValidationException, not served as an arbitrary page.
7. Mirror case: with the ids swapped (the foreign Outpost sorting first), the same replay silently ignores the cursor and re-serves the first page, so a client paging with it never advances.
