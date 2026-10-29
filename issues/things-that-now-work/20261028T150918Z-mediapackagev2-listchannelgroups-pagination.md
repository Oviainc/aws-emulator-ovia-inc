Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# A nextToken this service never issued is accepted as a page cursor, against the rule page_resume_after's own doc and the handler's own comment state

_Filed: 2026-10-28T15:09:18Z_
_Operation: ListChannelGroups | Class: pagination_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged list of this service decodes `NextToken` through the one shared helper `crate::state::page_resume_after`, and refuses the token with the modelled 400 `ValidationException` / `INVALID_PAGINATION_TOKEN` when that helper answers `None`. The helper's doc, and the comment in the `ListChannelGroups` handler above the call, both state the rule as "a token this service did not issue does not decode". The code does not implement that rule: `page_resume_after` answers `Some(..)` for ANY even-length hex string whose bytes are valid UTF-8, and the store keeps no record of the tokens it issued, so a token no list ever handed out is accepted as a cursor.

Because `page_token` is nothing but `hex::encode(last_name)`, the accepted-but-never-issued set is large and easy to hit by accident: a `NextToken` issued by one list is accepted verbatim by a DIFFERENT list (the tokens are plain hex of a resource NAME, with nothing naming the collection it came from), and any hand-written even-length hex run is accepted too. The page is then served starting strictly beyond a name that was never a page boundary of this collection, so the caller gets HTTP 200 with a silently truncated or empty `Items` instead of the refusal the crate says it makes:

* `GET /channelGroup?nextToken=6d` (hex of `m`) answers 200 and omits every channel group whose name sorts at or before `m` — and issues no `NextToken`, so the caller never learns anything was skipped;
* a `NextToken` read from a `ListChannels` response and passed to `ListChannelGroups` (a realistic client-side mixup, and exactly the case `INVALID_PAGINATION_TOKEN` exists for) is accepted and silently skips groups rather than being refused.

The four paged lists all share the same defect because they all share `page_resume_after`; `ListChannelGroups` is the first of them to call it.

## Evidence

The helper states the rule and does not implement it — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:3377-3388:
```
/// The pagination token for a list that stopped after `last` — opaque to the caller,
/// and decoded by [`page_resume_after`].
pub fn page_token(last: &str) -> String {
    hex::encode(last.as_bytes())
}

/// The name a pagination token resumes after, or `None` when the token is not one this
/// service issued.
pub fn page_resume_after(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}
```
`hex::decode` succeeds for every even-length hex run, so `"6d"` yields `Some("m")` — a cursor no list issued for this collection. Nothing else is consulted: the store holds no issued-token record (`ComAmazonawsMediapackagev2Mediapackagev2State` holds only `channel_groups`, src/state.rs:523-531).

The handler's own comment asserts the rule the helper breaks — src/ops/list_channel_groups.rs:184-199:
```
        // A token names the group the previous page stopped after. A token this service
        // did not issue does not decode, and is the modeled 400 the
        // `INVALID_PAGINATION_TOKEN` discriminator names — not a 501, which would tell an
        // SDK to retry a request that can never succeed.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::page_resume_after(token) {
                Some(after) => Some(after),
                None => {
                    return Err(ListChannelGroupsErrorModeled::ValidationException(
                        crate::state::invalid_pagination_token(),
                    )
                    .into());
                }
```
The same comment-and-call pair is in src/ops/list_channels.rs:207-219, src/ops/list_origin_endpoints.rs:255-267 and src/ops/list_harvest_jobs.rs:227-242.

What the accepted cursor then does — src/state.rs:3419-3432:
```
pub fn page<'a, T>(
    records: impl Iterator<Item = (&'a String, &'a T)>,
    resume_after: Option<&str>,
    size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let mut remaining = records
        .filter(|(name, _)| resume_after.is_none_or(|after| name.as_str() > after));
```
so a cursor of `"m"` drops every record whose name is `<= "m"` and the response is a 200.

Test coverage leaves the case open on purpose — tests/all/pagination_token_refusal.rs:10-14 and 135-144:
```
//! The table spans the class in both directions: every paged list the model declares
//! ... crossed with tokens that differ in FORM — prose, non-hex characters, an odd-length
//! hex run, something base64-shaped, and an even-length hex run that decodes to no text
//! at all.
...
    for token in [
        "not-a-real-pagination-token",
        "zzzz",
        "abc",
        "eyJrZXkiOiAidmFsdWUifQ==",
        "ff",
    ] {
```
Every row is a token the current decode already rejects on FORM; no row is an even-length hex run that decodes to text, which is exactly the accepted-but-never-issued case. So no existing test pins today's acceptance.

## Steps to Reproduce

1. Boot the emulator and create three channel groups: `aaa`, `mmm` and `zzz` (POST /channelGroup).
2. GET /channelGroup — all three are reported, and with no NextToken (the collection fits one page).
3. GET /channelGroup?nextToken=6d — `6d` is hex for `m`, a token no list ever issued for this collection. Observe HTTP 200 with Items containing only `mmm` and `zzz`: `aaa` was silently skipped, and no NextToken is reported, so the caller cannot tell anything is missing.
4. Compare with the rule the crate states: GET /channelGroup?nextToken=zzzz answers 400 ValidationException / INVALID_PAGINATION_TOKEN, because that token merely fails to decode — not because the service knows it did not issue it.
5. Cross-list mixup: GET /channelGroup/g/channel?maxResults=1, take the NextToken it issues (hex of a CHANNEL name), and send it to GET /channelGroup?nextToken=<that token>. Observe 200 with a silently truncated group listing instead of the modelled INVALID_PAGINATION_TOKEN refusal.
