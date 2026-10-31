Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: ab8b9af343344bd9ec1c9404b5c1438075827ba1

# A NextToken naming an Outpost installed in another Region is served as an arbitrary (often empty) page instead of being refused

_Filed: 2026-10-28T21:37:39Z_
_Operation: ListOutpostsWithS3 | Class: pagination_

> _Note: this issue was identified against the code base at commit `ab8b9af343344bd9ec1c9404b5c1438075827ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListOutpostsWithS3`'s cursor is the `OutpostId` the next page starts at, and the handler validates only its FORM: `decode_outpost_cursor` returns `Some` for any string matching `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`. The decoded id is then fed straight into the shared, Region-scoped walk.

The inventory is a `BTreeMap` keyed by `OutpostId` alone, and every Outpost belongs to exactly ONE home Region (`OutpostRecord::region`), which is also the only Region that reports it (`outpost_page`'s `|outpost| outpost.region == region`). So a token naming an Outpost installed in Region B is a token Region A's listing could never have minted — yet Region A honours it as a position in its own order. The walk's only use of the cursor is `if resume.as_ref().is_some_and(|from| key < from) { continue; }`, so:

* when the foreign Outpost's id sorts AFTER the ids installed in the requested Region, EVERY record of that Region is skipped and the handler answers HTTP 200 with `Outposts: []` and no `NextToken` — a confident "this account reaches no S3-capable Outpost in this Region" for a Region whose untokened listing reports several;
* when it sorts BEFORE, the cursor is a silent no-op and the FIRST page is re-served, so a client paging with it never advances.

Both outcomes are the one thing the handler's own comment and `state::list_cursor`'s doc say never happens, and they cross the Region boundary the crate states for every lookup and listing. The refusal machinery already exists here — `list_cursor` takes the `decode` closure and mints the `ValidationException` — so the gap is that the decoder knows nothing about the Region whose listing is resuming.

This is NOT the deleted-record tolerance the walk deliberately keeps (state.rs:1034-1035, "so a page is still served when the record the cursor named was deleted meanwhile"): the Outpost the token names is still present in the inventory, it simply belongs to a Region that, per this crate's own rule, reports nothing of it.

This is a different operation and a different decoder from the open `ListSharedEndpoints` pagination issue (which is about the cursor's Outpost half disagreeing with the request's `OutpostId` and lives in `decode_endpoint_cursor`); fixing that one does not touch `decode_outpost_cursor`.

## Evidence

The rule the crate states for every cursor — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:979-983:

    /// The key a listing resumes its walk at for `next_token`, as `decode` reads one
    /// out of a cursor: `None` for a request that carries no cursor, and the refusal
    /// for a token this service never issued — which is answered rather than served
    /// as an arbitrary page.

The handler repeats it — src/ops/list_outposts_with_s3.rs:156-160:

    // The cursor is the Outpost id the next page starts at. A token this
    // service never issued is refused rather than served as an arbitrary
    // page.
    let resume = crate::state::outpost_cursor(input.next_token.as_deref(), Self::NAME)
        .map_err(ListOutpostsWithS3ErrorModeled::ValidationException)?;

What the inventory cursor actually validates (the FORM only, nothing about the Region resuming) — src/state.rs:1049-1058:

    /// The Outpost id the inventory listing's cursor names, or `None` when the token
    /// is not one this service issued.
    ...
    pub fn decode_outpost_cursor(token: &str) -> Option<String> {
        is_outpost_id(token).then(|| token.to_string())
    }

The Region rule the token escapes — src/state.rs:56-63:

    //! resource here belongs to one Region and is reachable only through it:
    //! [`OutpostRecord::region`] records the Outpost's own (home) Region, ...
    //! and every lookup and listing resolves inside the Region the request was signed
    //! for alone ([`request_region`]) — so a resource of another Region is reported by
    //! no listing and resolves to the modelled 404, exactly as if the store held one
    //! set of resources per Region.

The two filters that collide in the shared walk — src/state.rs:1030-1044 and src/state.rs:1077-1091:

    for (key, record) in records {
        if !include(record) {
            continue;
        }
        // Resume at the first key at or after the cursor, so a page is still
        // served when the record the cursor named was deleted meanwhile.
        if resume.as_ref().is_some_and(|from| key < from) {
            continue;
        }

    pub fn outpost_page(
        inventory: &OutpostInventory,
        region: &str,
        ...
        list_page(
            inventory,
            max_results,
            resume,
            |outpost| outpost.region == region,

Test coverage of this seam: NONE. The only refused tokens are FORM failures — tests/all/endpoint_lifecycle.rs:632-644:

    Listing::Outposts => vec![
        ("no published Outpost id form at all", "not-a-cursor"),
        ("the other listing's endpoint cursor", "op-0a1b2c3d4e5f60718/0000000000000000001"),
        ("an Outpost id with non-hex digits", "op-zzzzzzzzzzzzzzzzz"),
        ("an Outpost id one character short", "op-0a1b2c3d4e5f6071"),
        ("an account id one digit short", "00000000000"),
        ("the ec2 form with a character added", "ec22"),
    ],

and tests/all/one_listing_rule.rs:110-137 (`every_listing_resumes_at_the_key_its_cursor_names`) only ever resumes with `listing.stored_keys(&engine)` — a cursor of the SAME listing in the SAME Region. tests/all/region_scoping.rs:173-198 checks the other Region's untokened listing only ("The other Region reports nothing of it, with no cursor promising more."), never a replayed cursor. So refusing a cursor whose Outpost is installed in another Region leaves every existing test passing.

## Steps to Reproduce

1. Stand up the emulator with one Outpost per Region, ids chosen so the foreign one sorts last — e.g. `emulator_holding(&[("op-00000000000000000".into(), "us-east-1"), ("op-fffffffffffffffff".into(), "us-west-2")])` (tests/all/endpoint_lifecycle.rs:71-87), or run with S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-00000000000000000:1024:<account>:us-east-1,op-fffffffffffffffff:1024:<account>:us-west-2".
2. GET /S3Outposts/ListOutpostsWithS3?maxResults=100 signed for us-east-1 — it reports op-00000000000000000.
3. GET /S3Outposts/ListOutpostsWithS3?maxResults=1 signed for us-west-2 and read the NextToken/id it reports there: op-fffffffffffffffff is an Outpost only us-west-2 serves.
4. Replay that id as this Region's cursor: GET /S3Outposts/ListOutpostsWithS3?maxResults=100&nextToken=op-fffffffffffffffff signed for us-east-1.
5. Observe HTTP 200 with {"Outposts": []} and no NextToken — the listing reports that the account reaches no S3-capable Outpost in us-east-1, although step 2 reported one. Expected per state.rs:979-983 and the handler's own comment: a token this listing never issued (and, being another Region's resource, could never issue) is refused with the 400 ValidationException, not served as an arbitrary page.
6. Mirror case: with the ids swapped (the foreign Outpost sorting first), the same replay silently ignores the cursor and re-serves the first page, so a client paging with it never advances.
