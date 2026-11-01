Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 66b81b2d38066f623e620cf9384f94dad963db50

# decode_cursor honours NextToken spellings encode_cursor never writes (upper/mixed-case hex, non-compact JSON), so a token this service never issued is served as a page instead of refused

_Filed: 2026-10-29T12:03:44Z_
_Operation: ListLenses | Class: pagination_

> _Note: this issue was identified against the code base at commit `66b81b2d38066f623e620cf9384f94dad963db50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate mints its `NextToken` through one encoder and reads it back through one decoder (`src/state.rs:8270-8284`). The encoder writes LOWERCASE hex of a COMPACT JSON object (`hex::encode(cursor.to_string())`); the decoder accepts anything `hex::decode` + `serde_json::from_slice` can parse. `hex::decode` is case-insensitive (it accepts `A`-`F` as well as `a`-`f`) and `serde_json` accepts any JSON spelling of the same object (added whitespace, extra members, reordered members), so a whole family of tokens the encoder can NEVER have written decodes to a valid cursor and is served as a page.

Concretely, take any `NextToken` this service issued and upper-case it: the uppercased string still satisfies the modelled `NextToken` `@pattern("^[A-Za-z0-9+/=_-]+$")`, so the boundary hands it to the handler, `decode_cursor` decodes it to the same cursor, and the handler answers `200` with a page. A caller can equally assemble `hex::encode("{\"c\": \"...\"}")` (one space added) — again not a string this service ever handed out — and be served a resume point.

That is the opposite of the rule the decoder's own doc states and of the rule each paginated handler implements and each test suite pins: a token this service did not issue must be a `400 ValidationException` classified `CANNOT_PARSE`, never a plausible page. The decoder is shared, so the defect reaches every paginated operation in the crate (ListLenses, ListWorkloads, ListMilestones, ListProfiles, ListShareInvitations, ListNotifications, the agent listings, …); it is reported on `ListLenses` because that handler and its suite spell the refusal rule out most explicitly. Fixing it in `decode_cursor` (reject a token that is not byte-identical to what `encode_cursor` would produce for the cursor it decoded to) repairs all of them at once and leaves every token the service issues working.

## Evidence

Side 1 — the decoder's own promise, and the encoder it contradicts: `src/state.rs:8270-8284` —
```
/// Encode a paging cursor as an opaque, self-describing `NextToken`.
///
/// Hex of the JSON cursor: it satisfies the modelled `NextToken` pattern
/// `^[A-Za-z0-9+/=_-]+$` and carries the whole cursor, so paging needs no
/// server-side token table — a reporting operation must not write to state.
fn encode_cursor(cursor: &::serde_json::Value) -> String {
    hex::encode(cursor.to_string())
}

/// The cursor a `NextToken` carries, or `None` when the token is not one this service
/// issued.
fn decode_cursor(token: &str) -> Option<::serde_json::Value> {
    let bytes = hex::decode(token).ok()?;
    ::serde_json::from_slice(&bytes).ok()
}
```
`hex::encode` emits lowercase digits only, while `hex::decode` accepts `A`-`F` too (`hex = { workspace = true }`, `Cargo.toml:27`), so `decode_cursor` answers `Some(..)` — "a token this service issued" — for a string `encode_cursor` cannot produce.

Side 2 — the rule the handler states for exactly this input: `src/ops/list_lenses.rs:193-210` — "An opaque token this service did not issue is a bad request, not an empty page. Classified `CANNOT_PARSE` like every other cursor in this service: a caller cannot correct a token it did not compose, only start the pagination again." followed by `match crate::state::decode_lens_next_token(token) { Some(cursor) => Some(cursor), None => return Err(... cannot_parse_exception(NEXT_TOKEN_MEMBER, INVALID_NEXT_TOKEN_MESSAGE) ...) }` — the refusal fires only when `decode_cursor` says `None`.

Side 2b — the crate's test suites pin the same rule, over rows that do not include a case-mutated hex spelling, so the gap is untested: `tests/all/lens_listing.rs:772-774` — "Rule 3's refusal: a `NextToken` this service did not issue is a 400 classified `CANNOT_PARSE` and naming the member, not an empty page. The rows are the ways a token can fail to be one of ours." (rows: `"not-a-token"`, `state::encode_milestone_next_token(1)`, `state::encode_next_token("0123…")`, `lens_listing.rs:778-790`). `tests/all/agent_recommendation_paging.rs:213-222` is the same shape — "Every row satisfies the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the boundary hands it to the handler: what is under test is the handler's own refusal" — with rows `not-a-cursor`, `hex-of-nothing`, `one-character`, `an-arn-like-token`, `a-context-cursor`, `a-workload-cursor`; no row upper-cases an issued token, which is why the divergence between the two helpers has gone unnoticed.

## Steps to Reproduce

1. Create at least two lenses' worth of catalog entries (the shipped official catalog alone is enough) and call `GET /lenses?MaxResults=1`; read the `NextToken` the page carries — it is lowercase hex, e.g. `7b2263223a…`
2. Upper-case that token verbatim (`token.to_uppercase()`). It still matches the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the request boundary accepts it
3. Call `GET /lenses?MaxResults=1&NextToken=<UPPERCASED TOKEN>`: the service answers `200` with the next page, although it never issued that string (`src/state.rs:8276` only ever writes lowercase hex)
4. Compare with the rows the suite pins (`tests/all/lens_listing.rs:778-790`), every one of which is answered `400 ValidationException` / `Reason: CANNOT_PARSE` naming `NextToken` — the answer this row should get as well
5. Equivalently, send `NextToken=<hex of `{"c": "<a catalog key>"}` with one space added>`: a string the encoder never produces is again served as a resume point
6. Repeat against any other paginated operation (e.g. `POST /workloadsSummaries` with an uppercased `NextToken`, `GET /api/v1/agent-profiles/{profileArn}/goals?nextToken=<uppercased>`) — they all read tokens through the same `decode_cursor`
