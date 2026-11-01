Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# A GetSearchResults nextToken is a bare row ordinal bound to no search, so a token minted for one search is honoured against another and silently hides its leading results

_Filed: 2026-10-29T09:05:12Z_
_Operation: GetSearchResults | Class: pagination_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetSearchResults pages its ranked result set by a NUMBER alone: `encode_token(resume)` is the hex of the decimal index, and `decode_token` parses that index back with no record of WHICH search (or which workspace) it was issued for. The only sanity check is `resume >= ranked.len()`, so a token minted while paging search A is accepted when replayed against a DIFFERENT search B in the same workspace whose result set is at least that long: B's first `resume` results are skipped and never appear on this page or on any later page of that enumeration, and the caller is answered 200 as if the page were real.

The same loose parse also honours spellings this handler never writes. `encode_token` only ever hexes the output of `usize::to_string`, but `decode_token` ends in `resume.parse().ok()`, and Rust's `usize::from_str` accepts a leading `+` and leading zeros — so hex("+3") and hex("007") are both served as position 3 although neither is a token this operation issued, exactly what the handler's own `invalid_token` doc says it refuses ("text that is not one of its tokens").

This crate states the opposite rule in two places of its own. Every paginated LISTING of this service pages through `crate::state::page_of`, whose resume point is an ENTRY IDENTITY and which refuses "a key no entry of this enumeration carries"; and the batch reader `BatchGetAssetPropertyValue` binds its positional token to a FINGERPRINT of the request it was issued for, documented as being there so "a token is only honoured against the request that produced it". GetSearchResults is the one search-side read where the position is bound to nothing.

## Evidence

The token this handler accepts, bound to no search:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_search_results.rs:239-253 —
    "        // `nextToken` is the index of the first result of the next page, hex-encoded
        // so the token stays opaque to the caller and matches the modelled
        // `^[A-Za-z0-9+/=]+$`. A token is issued only for a result this page HAS, so
        // an index at or past the end is one this operation never issued — including
        // every index at all for a search that matched nothing.
        let start = match input.next_token.as_deref() {
            Some(token) => {
                let resume = decode_token(token)?;
                if resume >= ranked.len() {
                    return Err(invalid_token());
                }
                resume
            }
            None => 0,
        };"
and what the two halves of the token actually carry — a decimal index and nothing else:
  src/ops/get_search_results.rs:522-536 —
    "/// A `nextToken` for the result the next page resumes from: its index in the ranked
/// enumeration, written out and hex-encoded so the token matches the modelled
/// `^[A-Za-z0-9+/=]+$`.
fn encode_token(resume: usize) -> String {
    ::hex::encode(resume.to_string().as_bytes())
}

/// Decode a `nextToken` back into the index it resumes from.
fn decode_token(token: &str) -> Result<usize, GetSearchResultsError> {
    ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|resume| resume.parse().ok())
        .ok_or_else(invalid_token)
}"
what the handler says such a token should get:
  src/ops/get_search_results.rs:538-540 — "/// HTTP 400 `InvalidRequestException` for a `nextToken` this operation never
/// issued: text that is not one of its tokens, or an index at or past the end of
/// the results it was issued for."

The crate's shared page walk, which refuses a token from another enumeration:
  src/state.rs:6712-6723 — "ONE recipe for every listing of this service that pages by a resume KEY taken off its entries, so the token format, the page size and what counts as a token this service never issued cannot drift between operations. `key` is the entry's resume key — unique within the enumeration and stable across calls ... `None` for a `next_token` this service never issued — text that is not the hex of a resume key, or a key no entry of this enumeration carries. Its caller answers that with the modelled refusal for an invalid `nextToken`, rather than with an empty page, which would report absence as success."
  src/state.rs:6735-6741 — "let start = match next_token { Some(token) => { let resume = decode_page_token(token)?; items.iter().position(|item| key(item).as_ref() == resume)? } None => 0, };"

The sibling read of this same crate that binds a POSITIONAL token to the request that minted it:
  src/ops/batch_get_asset_property_value.rs:386-401 — "The token carries the FINGERPRINT of the entry list beside the position, so a token is only honoured against the request that produced it: replayed against a different list it names no page, rather than silently re-paging that other list and reporting its leading entries as already processed." ... "fn decode_page_token(token: &str, entries: &[BatchGetAssetPropertyValueEntry]) -> Option<usize> { ... (fingerprint == entries_fingerprint(entries)).then_some(())?; at.parse().ok() }"

Test coverage: nothing pins the cross-search case. tests/all/get_search_results.rs:755-771 forges only malformed or out-of-range tokens — "A token this operation never issued is a bad request, not an empty page" with the rows ("not-a-token", "Zz"), ("past-the-end", hex_token("4")), ("far-past-the-end", hex_token("99")) — and tests/all/get_search_results.rs:689-751 walks the pages of ONE search only.

## Steps to Reproduce

1. CreateWorkspace W, then CreateDataset twice in W and associate several data segments to each, so two different searches can match different numbers of segments.
2. StartSearch in W with a query that matches at least 3 segments -> search S1; poll DescribeSearch until SUCCEEDED.
3. GET /workspaces/W/searches/S1/results?maxResults=1 -> 200 with a nextToken (hex of "1").
4. StartSearch in W with a different queryStatement / searchFilters that matches at least 3 other segments -> search S2; poll until SUCCEEDED.
5. GET /workspaces/W/searches/S2/results?maxResults=1 carrying S1's nextToken.
6. Observed: 200. S2's first (most relevant) result is skipped and is absent from this page and from every later page of this enumeration.
7. Expected per src/state.rs:6720-6723 (a key no entry of this enumeration carries is a token this service never issued) and src/ops/batch_get_asset_property_value.rs:386-389 (a token is only honoured against the request that produced it): the token names no page of S2 and is refused with InvalidRequestException.
8. Also: GET /workspaces/W/searches/S1/results?nextToken=<hex of "+1"> and <hex of "007"> are both answered 200 as position 1 and 7, although encode_token never writes either spelling.
