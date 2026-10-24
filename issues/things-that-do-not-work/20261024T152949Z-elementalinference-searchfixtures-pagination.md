Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# A SearchFixtures nextToken binds none of the search criteria, so a token from one search pages a different search

_Filed: 2026-10-24T15:29:49Z_
_Operation: SearchFixtures | Class: pagination_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SearchFixtures` is `@paginated(inputToken=nextToken, outputToken=nextToken, items=fixtures)` and its token is the hex encoding of the last `fixtureId` of the page and nothing else (`crate::state::encode_page_token`). The criteria that produced the page — `sport`, `startDate`, `endDate`, `filters`, `maxResults` — are not carried in the token and are not checked against it.

Consequences a client can observe:

1. A token issued for one search is honored for a DIFFERENT search. Page 1 of `sport=basketball` over one window, then page 2 requested with `sport=american-football` (or a different date window, or different `filters`) answers HTTP 200 with the second criteria's fixtures positioned after an id that belongs to the first result set — silently skipping part of the new selection, or reporting a page that belongs to no single query. A paginating client cannot tell this happened. A real paginated AWS search either encodes the query in the token or refuses a token that does not match the request (ValidationException).

2. Any even-length hex string that decodes to non-empty UTF-8 is accepted as a cursor (`decode_page_token`), so a token the service never issued — e.g. `nextToken="30"` (hex of "0") — is served as "resume after id 0" with HTTP 200 instead of being refused.

Expected: a `nextToken` is only valid for the request that produced it; a token that does not belong to the submitted criteria (or that the service never issued) is refused with the modeled `ValidationException` (HTTP 400). Actual: HTTP 200 with a page computed from mismatched criteria.

## Evidence

Spec: `smithy_query(query="operation",operation="SearchFixtures")` — paginated with inputToken/outputToken `nextToken`, `pageSize` `maxResults`; `ValidationException` (HTTP 400) is modeled. `SearchFixturesResponse$nextToken` is documented as the continuation of THIS search; well-established AWS pagination semantics is that a continuation token is only meaningful for the identical request parameters.
Handler: `src/ops/search_fixtures.rs` `handle()` lines ~295-342 — the token is decoded to a bare id and used only as `fixture.id > after` against the freshly computed `fixtures_in_window(sport, start_day, end_day)` selection; `crate::state::encode_page_token`/`decode_page_token` (`src/state.rs` ~1281-1295) carry nothing but the id, and are shared with ListFeeds/ListDictionaries.
Tests: `tests/all/fixture_catalog.rs` (~768-860) walks pages with UNCHANGED criteria and asserts only that a non-hex or empty token is a 400; no test submits a token together with criteria other than the ones that produced it, so the mismatch path is unasserted.

## Steps to Reproduce

1. POST /v1/fixtures with {"sport": "basketball", "startDate": <today>, "endDate": <today+1>, "maxResults": 1} -> HTTP 200 with one fixture and a nextToken
2. POST /v1/fixtures with {"sport": "american-football", "startDate": <today>, "endDate": <today+1>, "maxResults": 1, "nextToken": <the token from step 1>} -> observe HTTP 200 paging the american-football selection from a basketball fixture's position, instead of a 400 ValidationException for a token that does not belong to this search
3. POST /v1/fixtures with {"sport": "basketball", "startDate": <today>, "nextToken": "30"} (a token the service never issued) -> observe HTTP 200 instead of a 400
