Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 01d4c565076fad2daa34afee2942eef023c65029

# The page-size resolution rule and the region-scoped token criteria are each written out verbatim in every paginated handler

_Filed: 2026-10-27T08:05:43Z_
_Operation: ListFeeds | Class: maintainability_

> _Note: this issue was identified against the code base at commit `01d4c565076fad2daa34afee2942eef023c65029`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three paginated operations of this crate each re-implement the same two computations in their own file instead of sharing one definition with the rest of the pagination codec that already lives in `crate::state` (`page_criteria`, `encode_page_token`, `decode_page_token`).

1. Page-size resolution — "use maxResults, else the operation's default, floor at 1, clamp to MAX_PAGE_SIZE" — appears as the identical four-line expression in ListFeeds, ListDictionaries and SearchFixtures. Three copies of one rule can drift: a change to the floor or the clamp in one listing silently leaves the other two with the old rule.

2. Token criteria for the two region-scoped listings — `page_criteria(&[TOKEN_OPERATION, region])` — is written out once per file, so the rule "a listing's token binds its operation and its Region" exists twice.

A single helper in `crate::state` (taking the per-operation default/ceiling, and the per-operation token name) expresses each rule once. Every response and every existing test stay unchanged: the inputs, the defaults and the ceilings are per-operation constants that keep their current values.

## Evidence

Copy 1 — `src/ops/list_feeds.rs:234-237`:
```
        let page_size = input
            .max_results
            .map_or(DEFAULT_MAX_RESULTS, |max| max.max(1) as usize)
            .clamp(1, MAX_PAGE_SIZE);
```
Copy 2 — `src/ops/list_dictionaries.rs:219-222`:
```
        let page_size = input
            .max_results
            .map_or(DEFAULT_MAX_RESULTS, |max| max.max(1) as usize)
            .clamp(1, MAX_PAGE_SIZE);
```
Copy 3 — `src/ops/search_fixtures.rs:363-366`:
```
        let page_size = input
            .max_results
            .map_or(DEFAULT_MAX_RESULTS, |max| max.max(1) as usize)
            .clamp(1, MAX_PAGE_SIZE);
```
Token criteria, copy 1 — `src/ops/list_feeds.rs:149-151`: `fn token_criteria(region: &str) -> String { crate::state::page_criteria(&[TOKEN_OPERATION, region]) }`
Token criteria, copy 2 — `src/ops/list_dictionaries.rs:144-146`: `fn token_criteria(region: &str) -> String { crate::state::page_criteria(&[TOKEN_OPERATION, region]) }`

The codec these two rules belong beside is already shared: `src/state.rs:1991` `pub fn page_criteria(fields: &[&str]) -> String`, `:2021` `pub fn encode_page_token(...)`, `:2037` `pub fn decode_page_token(...)` — "Shared by every paginated operation of this service" (`src/state.rs:2019-2020`).

Existing tests that cover the behaviour and must stay green unchanged: `tests/all/pagination_token_binding.rs` (token binding across the three paginated operations), `tests/all/dictionary_listing.rs:142` ("An empty store still answers one page: an empty list and no token."), `tests/all/fixture_catalog.rs:825-836` (paging with explicit maxResults), `tests/all/feed_lifecycle.rs:343` (listing with a query string).

## Steps to Reproduce

1. Read src/ops/list_feeds.rs:234-237, src/ops/list_dictionaries.rs:219-222 and src/ops/search_fixtures.rs:363-366 — the same page-size resolution expression appears three times
2. Read src/ops/list_feeds.rs:149-151 and src/ops/list_dictionaries.rs:144-146 — the same region-scoped token-criteria body appears twice
3. Observe that the rest of the pagination codec is already shared in src/state.rs:1991-2058, so these two rules are the only parts of it duplicated per handler
