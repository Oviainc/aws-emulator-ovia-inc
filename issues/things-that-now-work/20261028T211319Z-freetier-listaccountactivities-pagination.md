Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 07b7ef72893d0cc8ecc84b280979d95c49b43697

# A nextToken carrying an offset past the end of the collection is answered with an empty 200 page instead of the ValidationException the crate raises for every other unissued token

_Filed: 2026-10-28T21:13:19Z_
_Operation: ListAccountActivities | Class: pagination_

> _Note: this issue was identified against the code base at commit `07b7ef72893d0cc8ecc84b280979d95c49b43697`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both paginated operations of this crate refuse a `nextToken` that is "not one <Operation> issued", and the crate's token codec is the single place that decides whether a token was issued. But `crate::state::decode_page_token` validates only the token's FORM (hex, version marker, list kind, a parsable `usize`); it never compares the offset it decodes against the collection the offset indexes into.

`crate::state::cut_page` is the only issuer of tokens, and it only ever encodes `position + 1` for a row it has just reported while a further selected row is still unreported. For the stored activity catalog (5 rows) that means the only offsets this service can ever hand out are offsets that name an existing, still-unreported row. A token such as `hex("ftv1:activities:5")` — or `...:999`, or any numeric offset at or past the end — is therefore a token `ListAccountActivities` can never have issued, yet `decode_page_token` accepts it, `.skip(offset)` selects nothing, `cut_page` issues no token, and the handler answers HTTP 200 with `{"activities": []}`.

So a corrupted, truncated or forged cursor whose numeric tail happens to parse is read as "the listing is exhausted" and reported as a SUCCESS, while the sibling malformations of the very same token (non-hex, hex of unrelated bytes, the right prefix with a non-numeric tail, the other listing's kind) are all refused with the modelled `ValidationException` (HTTP 400). The same single helper serves `GetFreeTierUsage` (`PAGE_KIND = "freeTierUsages"`, 6 stored records), so both listings share the hole; this is filed once against the first listing that calls the helper.

## Evidence

The check that decides whether a token was issued looks only at the token's form — services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:468-475:
```rust
/// The offset a `nextToken` resumes at, or `None` when the token was not issued
/// by [`encode_page_token`] for this `kind`.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let offset = text.strip_prefix(&format!("{PAGE_TOKEN_VERSION}:{kind}:"))?;
    offset.parse::<usize>().ok()
}
```

The only issuer of a token, which can only ever name a row that exists and is still unreported — src/state.rs:507-519:
```rust
pub fn cut_page<'rows, T>(
    kind: &str,
    rows: &'rows [(usize, T)],
    page_size: usize,
) -> (&'rows [(usize, T)], Option<String>) {
    let page = &rows[..page_size.min(rows.len())];
    let next_token = match page.last() {
        Some((position, _)) if rows.len() > page.len() => {
            Some(encode_page_token(kind, position + 1))
        }
        _ => None,
    };
```

The claim the handler makes about what the decode refuses — src/ops/list_account_activities.rs:193-205:
```rust
        // The token names the catalog position the next page begins at
        // (inclusive), so a page resumes exactly where its predecessor stopped.
        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => match crate::state::decode_page_token(PAGE_KIND, token) {
                Some(offset) => offset,
                None => {
                    return Err(validation(
                        "nextToken: the pagination token is not one ListAccountActivities issued",
                    ))
```
and the identical claim on the sibling listing, src/ops/get_free_tier_usage.rs:166-168: `"nextToken: the pagination token is not one GetFreeTierUsage issued"`.

Existing coverage stops one step short — tests/all/free_tier.rs:592-620:
```rust
/// A `nextToken` this service did not issue for this list is refused, not
/// silently read as page one.
...
    for token in [
        "not-a-token".to_string(),
        ::hex::encode("garbage"),
        ::hex::encode("ftv1:activities:not-a-number"),
        ::hex::encode("ftv1:freeTierUsages:1"),
        foreign,
    ] {
        refused(
            &router,
            "ListAccountActivities",
            &json!({"nextToken": token}),
            400,
            "ValidationException",
        )
```
Every probe is a token whose FORM is wrong; no row probes the right form with an offset the issuer can never produce. tests/all/free_tier.rs:1287-1306 (`an_unissued_usage_token_is_refused`) and tests/all/shared_rules.rs:86-163 (`both_paginated_operations_page_by_one_rule`, which only ever replays tokens the service itself issued) have the same blind spot.

## Steps to Reproduce

1. Boot the emulator with this service mounted (as tests/all/free_tier.rs::harness does) and call ListAccountActivities with {"maxResults": 1000} — the catalog holds 5 activities, so offsets 1..=4 are the only ones cut_page can ever encode.
2. Call ListAccountActivities with {"nextToken": hex::encode("ftv1:activities:5")} (or ":999"): observe HTTP 200 with "activities": [] and no nextToken, i.e. a token this service can never have issued is read as 'listing exhausted'.
3. Call ListAccountActivities with {"nextToken": hex::encode("ftv1:activities:not-a-number")}: observe HTTP 400 ValidationException "the pagination token is not one ListAccountActivities issued" — the same unissued token, refused only because its tail is not numeric.
4. Repeat step 2 against GetFreeTierUsage with hex::encode("ftv1:freeTierUsages:99"): same empty 200, because both listings decode through crate::state::decode_page_token.
