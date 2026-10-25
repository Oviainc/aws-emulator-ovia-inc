Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# ListOutpostsWithS3 hand-rolls the cursor refusal and page/token walk that state::endpoint_cursor and state::endpoint_page already own

_Filed: 2026-10-25T15:48:44Z_
_Operation: ListOutpostsWithS3 | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate implements one listing rule — "refuse a NextToken this service never issued with ValidationException naming the listing, then walk the store in key order resuming at or after the cursor, serve at most page_size records and return the first unserved key as NextToken" — twice. `crate::state::endpoint_cursor` + `crate::state::endpoint_page` hold the one copy the two endpoint listings share (the result of factoring out exactly this duplication once already), while `ListOutpostsWithS3` keeps its own inline copy of both halves. The two copies can drift: the shared helper's rule ("Resume at the first key at or after the cursor", token = first key not served) and the inline loop's version of it are maintained independently, and the refusal differs already (the shared one builds the message from the listing's `Self::NAME`, the inline one hardcodes the literal "ListOutpostsWithS3"). A helper in `state.rs` parameterised by the store map, cursor encode/decode and item projection would keep every response byte-identical and every existing paging test unchanged.

## Evidence

Copy 1 — the shared rule, services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:576-591 and :609-644:
```
pub fn endpoint_cursor(
    next_token: Option<&str>,
    operation: &str,
) -> Result<Option<(String, String)>, ValidationException> {
    match next_token {
        None => Ok(None),
        Some(token) => match decode_endpoint_cursor(token) {
            Some(key) => Ok(Some(key)),
            None => Err(ValidationException {
                message: Some(format!(
                    "NextToken {token:?} is not a token returned by {operation}"
                )),
```
```
    for (key, endpoint) in &store.endpoints {
        if !include(endpoint) { continue; }
        // Resume at the first key at or after the cursor, ...
        if resume.as_ref().is_some_and(|from| key < from) { continue; }
        if endpoints.len() == page_size {
            next_token = Some(encode_endpoint_cursor(key));
            break;
        }
        endpoints.push(endpoint.to_endpoint(region));
    }
```
Copy 2 — the same rule inline, src/ops/list_outposts_with_s3.rs:156-168 and :174-193:
```
        // The cursor is the Outpost id the next page starts at.
        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::is_outpost_id(token) {
                return Err(ListOutpostsWithS3ErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(format!(
                            "NextToken {token:?} is not a token returned by ListOutpostsWithS3"
                        )),
```
```
        for (outpost_id, outpost) in &store.outposts {
            // Resume at the first id at or after the cursor.
            if input.next_token.as_deref().is_some_and(|token| outpost_id.as_str() < token) { continue; }
            if outposts.len() == page_size {
                next_token = Some(outpost_id.clone());
                break;
            }
            outposts.push(outpost.to_outpost(&region));
        }
```
The crate's factoring precedent is stated in the helper's own doc, src/state.rs:606-608: "Each listing differs only in which endpoints it reports — `include` — so the walk itself lives here: both listings then page over one order, settle what the clock owes at the same point, and mint the same cursor for the same key."

Tests that must stay unchanged: tests/all/unprovisioned_account.rs::paging_carries_a_token_only_while_records_remain ("paging must terminate", "paging must enumerate every Outpost"), tests/all/unprovisioned_account.rs::an_unprovisioned_account_reports_no_outposts_and_no_token (cursor rows "?nextToken=op-0a1b2c3d4e5f60718"), tests/all/endpoint_lifecycle.rs::list_outposts_with_s3_pages_over_one_stable_order, and tests/all/endpoint_paging.rs::both_endpoint_listings_page_over_one_stable_order / ::both_endpoint_listings_refuse_a_token_they_never_issued for the shared copy.

## Steps to Reproduce

1. Read services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:576-591 and :609-644 — the cursor refusal and the resume/page-break/next-token walk, factored once for both endpoint listings
2. Read services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_outposts_with_s3.rs:156-193 — the same two blocks written again inline for the Outpost inventory
3. Observe the only differences are the key type (String vs (String, String)), the item projection and the hardcoded operation name in the refusal message, so the one listing rule is maintained in two places and can drift
