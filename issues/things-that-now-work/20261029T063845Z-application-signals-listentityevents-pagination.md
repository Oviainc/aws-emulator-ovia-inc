Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 2f84fd4c7d638614ea26115568d3a3113f96aac5

# The offset page cursor honours a POSITION this service never issued (0, "+N", leading-zero forms, or past the end), answering 200 instead of the unissued-token refusal

_Filed: 2026-10-29T06:38:45Z_
_Operation: ListEntityEvents | Class: pagination_

> _Note: this issue was identified against the code base at commit `2f84fd4c7d638614ea26115568d3a3113f96aac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every offset-paged read of this crate resolves its `NextToken` through `crate::state::page_of`, which validates only that the token is hex, carries the read's selection tag, and that whatever follows the tag `parse::<usize>()`-es. The POSITION itself is never checked against the positions this service can actually mint.

`page_of` only ever encodes `offset.saturating_add(page_size)` with `page_size = page_size.max(1)`, so every position it hands out is a canonical decimal >= 1. Three families of position are therefore values no listing ever produced, yet all three are honoured as a page boundary and answered 200:

* `0` — Rust's `usize::from_str` accepts it, and `items.into_iter().skip(0)` re-serves the FIRST page of the sequence. No page of any read can mint `0`.
* `+N`, `0N`, `00N` — `"+2".parse::<usize>()` and `"002".parse::<usize>()` are both `Ok(2)`, but `usize::to_string()` produces neither form, so neither is a token this service issued. They are silently read as position 2.
* a position past the end of the projection — served as an EMPTY page with no `NextToken`, so a client that replayed a stale/edited cursor is told the history is exhausted rather than that its token is not one this service issued.

The token is hex of plaintext (`encode_cursor` = `hex::encode(format!("{tag}{cursor}"))`, no MAC), so reaching these states needs no guesswork: hex-decode a token the read really issued, rewrite the digits after the tag, re-encode. The crate's own status-event suite already states exactly this threat model for the OTHER cursor type ("A token is opaque, not signed, so a caller can rewrite the payload inside the tag of a token it was issued") and that cursor's decoder validates the payload fields it carries; `page_of`'s does not.

The defect is in the shared helper, so it is the same on all eight offset-paged reads (`ListAuditFindings`, `ListEntityEvents`, `ListServices`, `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents`, `ListServiceStates`, `ListServiceLevelObjectiveExclusionWindows`); `ListEntityEvents` is named here as one operation whose handler calls it and whose own token suite leaves this half of the rule unpinned.

## Evidence

THE RULE THE HELPER STATES FOR ITSELF — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:1441-1452 (`page_of` doc):
```
/// Each read
/// passes its own [`SERVICE_OPERATIONS_TOKEN_TAG`]-style tag, so a token is only ever
/// resolved against the projection that issued it; `Err` carries the refusal message for a
/// token that is not one this service issued under that tag — the service's ONE wording for
/// that failure ([`UNISSUED_NEXT_TOKEN`]), so a cursor refused here reads exactly like the
/// same cursor refused by a read whose cursor is a stored key.
```
and src/state.rs:1342-1348 (`decode_next_token`): "which covers a token that is not hex, is not UTF-8, or is well-formed but carries no [`NEXT_TOKEN_TAG`], so a forged token is refused rather than honoured as a page boundary."

THE CODE THAT DOES NOT CHECK THE POSITION — src/state.rs:1465-1478:
```
    let page_size = page_size.max(1);
    let offset = match token {
        None => 0,
        Some(token) => decode_cursor(tag, token)
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .ok_or_else(|| UNISSUED_NEXT_TOKEN.to_string())?,
    };
    let mut page: Vec<T> = items.into_iter().skip(offset).collect();
    let next_token = if page.len() > page_size {
        page.truncate(page_size);
        Some(encode_cursor(tag, &offset.saturating_add(page_size).to_string()))
```
(`page_size >= 1` and `offset >= 0`, so the ONLY positions ever minted are canonical decimals `>= 1`.)

THE TOKEN IS PLAINTEXT, SO THE POSITION IS CALLER-EDITABLE — src/state.rs:1350-1361:
```
pub fn encode_cursor(tag: &str, cursor: &str) -> String {
    ::hex::encode(format!("{tag}{cursor}"))
}
...
pub fn decode_cursor(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}
```

THE SIBLING CURSOR IN THIS SAME MODULE THAT DOES CHECK ITS PAYLOAD — src/state.rs:4042-4056 (`StatusEventCursor::decode`):
```
    /// A `NextToken` is opaque but not authenticated, so every field it carries is a value a
    /// caller may have written: an instant is accepted only when its subsecond part is a real
    /// one (strictly under a second, [`NANOS_PER_SECOND`]), which is the range
    /// [`Self::window`]'s timestamps are defined over. A payload carrying anything else is not
    /// a cursor this service issued, and is refused exactly like a token that is not hex.
    pub fn decode(payload: &str) -> Option<Self> {
...
            let nanos = nanos.parse::<u32>().ok().filter(|nanos| *nanos < NANOS_PER_SECOND)?;
```

THE CALL SITE — src/ops/list_entity_events.rs:226-232:
```
        let (change_events, next_token) = crate::state::page_of(
            observed,
            &token_tag,
            input.next_token.as_deref(),
            input.max_results.max(0) as usize,
        )
        .map_err(refused)?;
```

TEST COVERAGE — the existing suites pin the TAG half of the rule and non-numeric payloads only, never the position:
* tests/all/unissued_next_token.rs:261-288 enumerates nine forged shapes (empty, not hex, odd hex, non-UTF-8, no marker, "a marker of this service with no position", over-long, credential-shaped, a token issued for another read) — every one fails at the hex/tag step, none carries a valid tag with an invented NUMBER;
* tests/all/entity_events.rs:665-680 is the same four shapes for this read (`hex::encode("entity-events/1/first")` fails the tag prefix, not the position parse);
* tests/all/paged_cursor_selection_binding.rs:6-13 and tests/all/paged_cursor_subject_binding.rs pin only that a token from another SELECTION is refused;
* tests/all/instrumentation_configuration_status.rs:1029-1046 is the contrast case — it rewrites the payload INSIDE a legitimately issued token (`retagged_token(&token, "0.2000000000/0.0/0.0")`) and asserts 400, the very assertion no offset-paged read has.

## Steps to Reproduce

1. Start the emulator with this crate mounted. POST /slo three times to create three SLOs whose SliConfig key attributes are {"Type":"Service","Name":"checkout","Environment":"prod"}, so the entity holds three change records.
2. POST /events?MaxResults=1 with body {"Entity":{"Type":"Service","Name":"checkout","Environment":"prod"},"StartTime":<now-3600>,"EndTime":<now+3600>} — the answer is one ChangeEvent plus a NextToken.
3. hex-decode that NextToken: the plaintext is `entity-events/1/<hash>/1`. Replace the trailing `1` with `0` and hex-encode again.
4. Replay POST /events?MaxResults=1&NextToken=<forged> with the same body: the read answers 200 and serves the FIRST event again, although no page of this (or any) read can mint position 0 (`page_of` encodes `offset + page_size.max(1)`, always >= 1). Expected: HTTP 400 ValidationException with crate::state::UNISSUED_NEXT_TOKEN, as src/state.rs:1441-1452 says `page_of` answers for a token this service did not issue.
5. Repeat with the payload rewritten to `+1` and to `01`: both answer 200 and serve position 1, although `usize::to_string()` produces neither spelling.
6. Repeat with the payload rewritten to `900`: the read answers 200 with an empty ChangeEvents list and no NextToken, so a client reading a never-issued cursor is told the history is exhausted instead of being refused.
7. Contrast with the status read, whose cursor validates its rewritten payload: tests/all/instrumentation_configuration_status.rs:1029-1046 asserts 400 for exactly this attack on GetInstrumentationConfigurationStatus.
