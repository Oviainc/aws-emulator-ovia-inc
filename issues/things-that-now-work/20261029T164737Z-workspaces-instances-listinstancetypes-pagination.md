Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a399f03ccc4f5afa40c0af0ce737308e8fb96c4f

# decode_next_token honours uppercase/mixed-case hex NextToken spellings encode_next_token never mints, so a token this service never issued resumes a page

_Filed: 2026-10-29T16:47:37Z_
_Operation: ListInstanceTypes | Class: pagination_

> _Note: this issue was identified against the code base at commit `a399f03ccc4f5afa40c0af0ce737308e8fb96c4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has ONE pagination-token encoding: `encode_next_token` hex-encodes the resume point and `decode_next_token` is documented as its "exact inverse". `hex::encode` only ever emits LOWERCASE hex digits, but `hex::decode` accepts uppercase and mixed-case digits too, so the decode side honours an infinite family of spellings the encode side can never mint.

The consequence is on both paginated listings that mint a token. A caller who uppercases the hex of a token it was handed — or who fabricates one from scratch, e.g. `hex::encode_upper("wsinst-0000000000000001")` for `ListWorkspaceInstances`, or `hex::encode_upper("c5.large")` for `ListInstanceTypes` — presents a NextToken this service never issued. Both handlers decode it, resolve it to a resume point and serve a 200 page from that position, instead of the `ValidationException` naming `NextToken` that the crate single-homes in `invalid_next_token` for exactly this case. `ListRegions` is unaffected only because it refuses every token outright.

The gap is in the decoder's acceptance set, not in the resume-point lookup: the lookup can only tell a resume point it cannot NAME from one it can, and an uppercase re-spelling decodes to the very same, nameable, resume point.

## Evidence

The encode side mints lowercase only, and the crate states the two are inverses — `src/state.rs:2344`:
```
/// Encode a resume point into the `NextToken` a paginated operation hands back: the
/// exact inverse of [`decode_next_token`], so a token this service mints always
/// decodes on the follow-up request. Mint every token through this.
pub fn encode_next_token(resume_point: &str) -> String {
    hex::encode(resume_point)
}
```
The decode side accepts any hex case — `src/state.rs:2359`:
```
pub fn decode_next_token(token: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|raw| String::from_utf8(raw).ok())
}
```
and its own doc states the rule it is supposed to enforce (`src/state.rs:2351-2357`): "Decode a `NextToken` this service issued back to the resume point it carries. … so a token that does not decode (or decodes to a resume point the caller's list cannot name) is refused with [`invalid_next_token`] rather than guessed at." `invalid_next_token` is documented (`src/state.rs:2365-2367`) as "The modelled `ValidationException` (HTTP 400) for a `NextToken` this service did not issue".

Both consumers go straight from that decoder to a served page — `src/ops/list_instance_types.rs:182`:
```
            Some(token) => {
                let decoded = crate::state::decode_next_token(token)
                    .and_then(|name| crate::state::instance_type_index(&name));
```
and `src/ops/list_workspace_instances.rs:181`:
```
            Some(token) => {
                let decoded = crate::state::decode_next_token(token)
                    .and_then(|id| state.instance_resume_point(&region, &id));
```

Test coverage: `tests/all/next_token_rule_sharing.rs:9-12` states the rule as "a `NextToken` this service did not issue is a property of the TOKEN, not of the list it was presented to: every paginated operation of the service refuses it …", and its `token_rows()` (lines 96-128) exercises only lowercase `hex::encode(...)` forms plus non-hex junk — no uppercase or mixed-case spelling is driven, so the gap is unpinned.

## Steps to Reproduce

1. POST / with `X-Amz-Target: EUCMIFrontendAPIService.ListInstanceTypes` and body `{"MaxResults": 1}`; note the returned `NextToken` (lowercase hex of the second catalogued instance type name).
2. Re-send `{"MaxResults": 1, "NextToken": "<that token with every hex letter upper-cased>"}`. Observed: HTTP 200 resuming the page. Expected: HTTP 400 `ValidationException` from `crate::state::invalid_next_token`, because `encode_next_token` (`hex::encode`) never mints an uppercase digit, so that token was never issued by this service.
3. Repeat against `ListWorkspaceInstances`: `CreateWorkspaceInstance` once, then send `{"NextToken": "<hex::encode_upper of the returned wsinst- id>"}`. Observed: HTTP 200 page starting at that record; expected: the same 400 the crate's `tests/all/next_token_rule_sharing.rs` asserts for a token the service did not issue.
