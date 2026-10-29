Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a1e71609e4dd8609a7a875072f755c7dbeedcd71

# ListMonitors refuses an EMPTY nextToken with 400 instead of serving the first page

_Filed: 2026-10-29T13:16:49Z_
_Operation: ListMonitors | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a1e71609e4dd8609a7a875072f755c7dbeedcd71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListMonitorsInput$nextToken` targets `PaginationToken`, whose modelled bound is `@length(min: 0, max: 4096)` — the empty string is a value the model admits, so the framework hands it to the handler. The handler then sends it through `crate::state::decode_page_token`, which requires the cursor marker, so `""` decodes to `None` and the whole listing is refused with a 400 `ValidationException` saying the token was never vended by a previous ListMonitors call. A caller that sends `?nextToken=` (a paginator that initialises its cursor to the empty string) gets an error instead of the first page, even though it named no position.

This crate elsewhere states the opposite rule for the same kind of modelled bound: `UntagResource`'s handler comments that an EMPTY `tagKeys` list, being inside the modelled `@length(min: 0)`, is "a request that removes nothing rather than a bad request" — and it is answered as a no-op success. ListMonitors applies the inverse to a member whose model also admits the empty value, turning it into a refusal. The empty token names no position in the key order, which is exactly the `start = 0` case the handler already has for an absent token.

## Evidence

Model (smithy_query shape=ListMonitorsInput): `"nextToken": {"target": "PaginationToken", "httpQuery": "nextToken", "length": {"min": 0, "max": 4096}, "doc": "<p>The token for the next page of results.</p>"}` — min length 0, so `""` passes boundary validation and reaches the handler.

The refusal — `services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/list_monitors.rs:212-231`:
```
let start = match input.next_token.as_deref() {
    None => 0,
    Some(token) => {
        let Some(key) = crate::state::decode_page_token(token) else {
            return Err(validation(crate::state::unknown_page_token_refusal(
                "nextToken", Self::NAME, token,
            )));
        };
```
and `src/state.rs:419-422`:
```
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_MARKER).map(str::to_owned)
}
```
`hex::decode("")` succeeds and yields `""`, which carries no `PAGE_TOKEN_MARKER`, so `""` -> `None` -> 400.

The sibling rule this crate states for a model-admitted empty value — `src/ops/untag_resource.rs:181-184`:
```
// ... An
// EMPTY `tagKeys` is inside the modelled `@length(min: 0)`, so it is a
// request that removes nothing rather than a bad request.
```

Test coverage: `tests/all/list_monitors_pagination.rs:328-354` `a_next_token_this_service_never_vended_is_refused` pins only three rows — `"not-a-token"`, the hex of text nothing vended, and the bare hex of a stored `monitorName`; no row sends the empty string, so no test states a reason for refusing it.

## Steps to Reproduce

1. CreateMonitor(monitorName="mon-a") and CreateMonitor(monitorName="mon-b").
2. GET /monitors?nextToken= (an empty nextToken, which the modelled @length(min: 0) admits).
3. Observe HTTP 400 ValidationException ("Member must be a nextToken returned by a previous ListMonitors call") instead of the first page of monitors.
4. Control: GET /monitors (nextToken absent) returns 200 with both monitors, which is the page the empty token names.
