Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c73cb477601ff7c7855bce46e4f89627c43b834e

# Pagination tokens never expire, so an expired nextToken is answered HTTP 200 instead of the documented 400

_Filed: 2026-10-29T12:38:50Z_
_Operation: ListIngestions | Class: pagination_

> _Note: this issue was identified against the code base at commit `c73cb477601ff7c7855bce46e4f89627c43b834e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing in this crate mints its `nextToken` through `crate::state::encode_page_token`, which renders only a version prefix, the listing name and a position (`v1-{listing}-{sequence:016x}`). The token therefore carries NO issue time, and `decode_page_token` checks only the prefix and that the remainder parses as hex. Nothing in the crate can tell a token minted a minute ago from one minted a week ago, so a token the model declares expired is accepted forever and answered HTTP 200 with a page of results.

The model states the opposite, in the documentation of the `nextToken` member of this operation (and of its three siblings): the token expires after 24 hours and a call carrying an expired one is refused with an HTTP 400 error. The handler's own generated contract header restates that sentence verbatim, so the rule is written into the file whose code ignores it.

This is NOT the gap already filed as `20261029T055155Z-appfabric-listappbundles-pagination`: that issue is about a well-formed token naming a position the listing never issued (sequence 0, or past the counter) being read as a position. This one is about a token the listing DID issue being honoured after the lifetime the model gives it. Both go through the same helper, but they are different checks on different inputs.

The gap is in the shared helper, so all four paginated operations behave this way (`ListIngestions`, `ListIngestionDestinations`, `ListAppAuthorizations`, `ListAppBundles`); `ListIngestions` is named here because its own module header carries the quoted sentence twice and its handler passes the caller's token straight into `page()`.

## Evidence

The model's own sentence — smithy_query(query="shape", shape="ListIngestionsResponse"), member `nextToken`:

    "nextToken": {"target": "String", "doc": "<p>If <code>nextToken</code> is returned, there are more results available. ... Keep all other arguments unchanged. Each pagination token expires after 24 hours. Using an expired pagination token will return an <i>HTTP 400 InvalidToken error</i>.</p>"}

The same sentence restated in the handler file — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/list_ingestions.rs:60-64:

    //! * `nextToken` (String) — If nextToken is returned, there are more results available. The
    //!   value of nextToken is a unique pagination token for each page. Make the call again using
    //!   the returned token to retrieve the next page. Keep all other arguments unchanged. Each
    //!   pagination token expires after 24 hours. Using an expired pagination token will return
    //!   an HTTP 400 InvalidToken error.

What the token actually contains — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs:1516-1518:

    pub fn encode_page_token(listing: &str, sequence: u64) -> String {
        format!("v1-{listing}-{sequence:016x}")
    }

— a listing name and a position, and no instant.

What a later call checks — src/state.rs:1522-1529:

    pub fn decode_page_token(listing: &str, token: &str) -> Result<u64, ValidationException> {
        token
            .strip_prefix(&format!("v1-{listing}-"))
            .and_then(|sequence| u64::from_str_radix(sequence, 16).ok())
            .ok_or_else(|| {
                field_validation_failed("nextToken", "nextToken is not a valid pagination token")
            })
    }

Caller side (unchanged by this finding) — src/ops/list_ingestions.rs:186-194 hands `input.next_token` to `crate::state::page`, which decodes it with `decode_page_token` (src/state.rs:1477-1480).

Nothing in the crate justifies the divergence: grepping the crate for `expire|expiry|24 hours|InvalidToken` matches ONLY the four generated List handler doc headers (list_ingestions.rs:63-64 and :72-73, list_app_bundles.rs:58-59 and :68-69, list_app_authorizations.rs:66-67 and :76-77, list_ingestion_destinations.rs:77-78 and :87-88) and the model JSON — no comment, no constant and no test mentions token lifetime, while the crate does already date other clock-owed transitions (`IngestionRecord::disables_at`, src/state.rs:638-642; `UserAccessTaskRecord::settles_at`, src/state.rs:511-515).

Test coverage: the pagination tests pin ordering, page size and refusal of malformed/cross-listing tokens (tests/all/app_bundle_ingestion_lifecycle.rs:630-631 "RULE: a `nextToken` the service did not issue is refused as a `ValidationException`", :668 cross-listing) — none of them covers a token's age, so no existing expectation is attached to accepting an old token.

## Steps to Reproduce

1. Boot the emulator, create an app bundle, an app authorization (connected) and two ingestions under it.
2. Call `GET /appbundles/{bundleUuid}/ingestions?maxResults=1` and keep the returned `nextToken` (it reads `v1-ingestions-<bundleUuid>-0000000000000003` or similar — a listing name and a position, with no issue time).
3. Observe that no later call can distinguish that token's age: `decode_page_token` only strips the prefix and parses the hex position.
4. Replay the same token any amount of time after it was issued (e.g. after 24 hours of wall-clock, or in a process where the clock has been moved forward). Observed: HTTP 200 with the next page. Expected per the member's own documentation: an HTTP 400 refusal for an expired pagination token.
5. Repeat for `ListAppBundles`, `ListAppAuthorizations` and `ListIngestionDestinations`, which decode through the same helper, and see the same 200.
