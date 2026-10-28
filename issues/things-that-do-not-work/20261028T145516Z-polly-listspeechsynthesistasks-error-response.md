Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 41cd94e3f39433730586ca0c36d19074975940e2

# ListSpeechSynthesisTasks refuses the empty NextToken with InvalidNextTokenException, while the crate's own rule is that an empty token reads as no token

_Filed: 2026-10-28T14:55:16Z_
_Operation: ListSpeechSynthesisTasks | Class: error-response_

> _Note: this issue was identified against the code base at commit `41cd94e3f39433730586ca0c36d19074975940e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for the empty `NextToken` the model admits (`NextToken` is len 0..4096): it names no position and reads as no token at all. `ListLexicons` and `DescribeVoices` both apply it through the shared helper `crate::state::is_unmintable_page_token`, which refuses only a NON-empty token, and both have tests pinning `?NextToken=` as a normal 200 response.

`ListSpeechSynthesisTasks` does not use that helper. It sends every `Some(token)` — including `Some("")`, which is what the framework binds `?NextToken=` to — into `decode_task_cursor`, where the first field of the decoded cursor is parsed as an `i64`; for the empty cursor that parse fails, the function answers `None`, and the handler returns HTTP 400 `InvalidNextTokenException`. So the same empty query parameter is a successful first page on two list operations of this service and a 400 on the third, for no reason stated anywhere in the crate.

## Evidence

The crate's rule and the helper that implements it — services/service-com-amazonaws-polly-parrot-v1/src/state.rs:475-485:
```
/// Whether `token` is a continuation token an UNPAGINATED read was handed.
/// ... The empty token the model admits (`NextToken` is len 0..4096) names
/// no position, so it reads as no token at all.
pub fn is_unmintable_page_token(token: Option<&str>) -> bool {
    token.is_some_and(|token| !token.is_empty())
}
```
Applied by the siblings — src/ops/list_lexicons.rs:158 `if crate::state::is_unmintable_page_token(input.next_token.as_deref()) {` and src/ops/describe_voices.rs:189 `if crate::state::is_unmintable_page_token(input.next_token.as_deref()) {` — and pinned by tests/all/lexicon_listing.rs:237-241:
```
    let empty = json(&polly.list_lexicons("?NextToken=").await);
    ... "case \"?NextToken=\": an empty token must read as no token"
```
and tests/all/voice_catalog_is_not_paged.rs:138-142:
```
    let empty = voices(&polly, "?NextToken=").await;
    ... "case \"?NextToken=\": an empty token must read as no token, got {} voices"
```

The path that skips it — src/ops/list_speech_synthesis_tasks.rs:234-243:
```
        let after = match input.next_token.as_deref() {
            Some(token) => Some(
                decode_task_cursor(token, input.status.as_deref()).ok_or_else(|| {
                    ListSpeechSynthesisTasksErrorModeled::InvalidNextTokenException(
                        crate::service::errors::invalid_next_token(),
                    )
                })?,
            ),
            None => None,
        };
```
with src/ops/list_speech_synthesis_tasks.rs:166-171 showing the empty cursor cannot decode:
```
fn decode_task_cursor(token: &str, status: Option<&str>) -> Option<(i64, String)> {
    let cursor = crate::state::decode_page_token(token)?;
    let mut fields = cursor.split(CURSOR_SEPARATOR);
    let created_millis = fields.next()?.parse::<i64>().ok()?;
```
(`decode_page_token("")` succeeds — `hex::decode("")` is `Ok([])`, state.rs:471-473 — so the failure is the `"".parse::<i64>()`.)

That `?NextToken=` really reaches the handler as `Some("")` — vendor/smithy-schema-server/src/http_binding.rs:103-110:
```
        if !query_str.is_empty() {
            for pair in query_str.split('&') {
                let mut it = pair.splitn(2, '=');
                let k = percent_decode_query(it.next().unwrap_or(""));
                let v = percent_decode_query(it.next().unwrap_or(""));
                query.entry(k.clone()).or_insert_with(|| v.clone());
```

Test coverage: tests/all/synthesis_tasks.rs:311-327 `a_task_page_token_only_resumes_its_own_listing` enumerates the refused tokens (`not-a-token`, `zzzz`, `ff`, a foreign hex cursor) and does NOT include the empty token, so no test pins the 400 this handler answers for it.

## Steps to Reproduce

1. GET /v1/lexicons?NextToken= → HTTP 200 with the Region's lexicons (the empty token reads as no token).
2. GET /v1/voices?NextToken= → HTTP 200 with the whole catalog (same rule).
3. GET /v1/synthesisTasks?NextToken= → HTTP 400 `InvalidNextTokenException` instead of the first page, because the empty token is pushed through `decode_task_cursor` rather than read as absent.
