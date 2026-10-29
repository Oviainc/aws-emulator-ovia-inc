Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0f3666ffc56d576d343f5b2efcf2fb2c8ab1775d

# decode_page_token accepts hex spellings ListRegions never mints, so a client-fabricated NextToken resumes a page instead of being refused

_Filed: 2026-10-29T15:14:45Z_
_Operation: ListRegions | Class: pagination_

> _Note: this issue was identified against the code base at commit `0f3666ffc56d576d343f5b2efcf2fb2c8ab1775d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRegions` mints its `NextToken` with `::hex::encode(region_name)`, which emits LOWER-CASE hex only. `decode_page_token` accepts the token back through `::hex::decode`, which also accepts upper- and mixed-case hex digits, and then only checks that the decoded bytes name a catalog Region. So a token this operation could never have issued — e.g. the upper-case spelling `61662D736F7574682D31` of `af-south-1` — decodes successfully and is served as a resume point (HTTP 200, the page after `af-south-1`), instead of the `ValidationException` the handler's own doc and its `invalid_next_token()` payload promise for a token "minted elsewhere".

The fix is a round-trip check of the kind this repo already uses elsewhere (`encode_page_token(&decoded) != token` ⇒ `None`); nothing about the tokens the operation itself issues changes, so every existing paging test keeps passing.

## Evidence

The documented rule and the code that breaks it, in the same file:

services/service-com-amazonaws-account-account/src/ops/list_regions.rs:114-130
```
/// Encode the resume point of a page as the opaque `NextToken` the caller hands back.
...
fn encode_page_token(region_name: &str) -> String {
    ::hex::encode(region_name)
}

/// The Region code a `NextToken` resumes after, or `None` when the token is not one
/// this operation could have issued — one minted elsewhere resumes nothing here and is
/// refused rather than silently ignored.
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    crate::state::catalog_region_status(&decoded).map(|_| decoded)
}
```
There is no check that `token` is the spelling `encode_page_token` would have produced, and the handler feeds the result straight into the resume filter (list_regions.rs:244-249, 285-289).

That `hex::decode` is case-lenient is stated in this repo, by a sibling service that guards against exactly this:
services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:1645-1656
```
/// ... and a spelling of
/// the hex that differs from the one this service hands out (`hex::decode` also accepts
/// upper case). Either one would otherwise resume a walk from a position the service
/// never offered.
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    ...
    if value.is_empty() || encode_cursor(kind, value) != token {
        return None;
    }
```

Test coverage: services/service-com-amazonaws-account-account/tests/all/region_opt_in_listing.rs:659-688 (`a_foreign_next_token_is_refused`) pins the 400 for "a word", "a Region code in the clear", "an encoding of a Region nobody serves", "odd-length hex" and "the empty token" — the one spelling it does not try is the upper-case hex of a real Region, which is the case that slips through.

## Steps to Reproduce

1. Start the emulator with the account service mounted (as tests/all/region_opt_in_listing.rs does).
2. POST /listRegions with body {"NextToken": "61662D736F7574682D31"} — the UPPER-CASE hex of "af-south-1", a spelling encode_page_token never emits.
3. Observed: HTTP 200 with the page of Regions following af-south-1 — the fabricated token resumed a walk.
4. Expected per list_regions.rs:124-126 and invalid_next_token(): HTTP 400 ValidationException, "NextToken is not a pagination token that ListRegions issued", exactly as for the "odd-length hex" and "a Region code in the clear" cases the suite already pins.
5. For contrast, POST /listRegions with {"MaxResults": 1}, take the issued NextToken, and note it is all lower-case hex.
