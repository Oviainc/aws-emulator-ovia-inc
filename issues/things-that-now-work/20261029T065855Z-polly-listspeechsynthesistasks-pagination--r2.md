Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 92ec46034f67b146e2b05eb49583adb1c148c1c8

# A hand-crafted three-field NextToken is honoured as a page position the service never minted

_Filed: 2026-10-29T06:58:55Z_
_Operation: ListSpeechSynthesisTasks | Class: pagination_

> _Note: this issue was identified against the code base at commit `92ec46034f67b146e2b05eb49583adb1c148c1c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`decode_task_cursor` validates only the SHAPE of a continuation token: it hex-decodes it, splits on the unit separator, parses the first field as an `i64`, takes the second as a task id and compares the third against the request's `Status`. Nothing ties the decoded value to a cursor this service ever issued. Any caller can therefore construct `hex("<millis>\u{1f}<anything>\u{1f}")` and get HTTP 200 with the report resumed at an arbitrary creation instant — the first page for `millis = 0`, an empty page for a `millis` in the future — instead of the `InvalidNextTokenException` the handler's own comment, the shared decoder's doc and the crate's own test rule all promise for a token the service did not mint.

The existing unit test and end-to-end test only exercise forgeries that fail the SHAPE check (non-hex, hex that decodes to fewer than three fields), so the hole sits exactly where the stated rule is not exercised.

## Evidence

The rule the crate states for itself, three times:

`src/ops/list_speech_synthesis_tasks.rs:232-238` (handler):
```
// A token that names a position must be one this service minted, for this
// request's filter; anything else is refused before any state is read, so a
// garbage or foreign token can never read as "first page".
```

`src/state.rs:469-473`:
```
/// Decode a `NextToken` produced by [`encode_page_token`]; `None` for any token
/// this service did not mint.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}
```
— the body can only tell hex-of-UTF-8 from not-hex; it cannot tell minted from forged.

`tests/all/synthesis_tasks.rs:286-288`:
```
/// RULE: a `NextToken` resumes the listing it was issued for and no other — a token
/// this service did not mint, or one minted under a different `Status` filter, is
/// `InvalidNextTokenException` (HTTP 400) rather than a page of the wrong tasks.
```

What the code actually accepts — `src/ops/list_speech_synthesis_tasks.rs:166-178`:
```
fn decode_task_cursor(token: &str, status: Option<&str>) -> Option<(i64, String)> {
    let cursor = crate::state::decode_page_token(token)?;
    let mut fields = cursor.split(CURSOR_SEPARATOR);
    let created_millis = fields.next()?.parse::<i64>().ok()?;
    let task_id = fields.next()?.to_string();
    let minted_for = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    (minted_for == status.unwrap_or_default()).then_some((created_millis, task_id))
}
```

Test coverage that stops short of the hole — `tests/all/synthesis_tasks.rs:314-318` only sends two-field / non-hex forgeries:
```
"?NextToken=not-a-token".to_string(),
"?NextToken=zzzz".to_string(),
"?NextToken=ff".to_string(),
// Well-formed hex, but not a cursor this operation ever minted.
format!("?NextToken={}", hex::encode("1\u{1f}x")),
```
and the in-crate unit test `task_cursors_round_trip_and_reject_foreign_tokens` (`src/ops/list_speech_synthesis_tasks.rs:358`) likewise only feeds `["not-hex", "abc", "ff", "fffe", &::hex::encode("1")]`.

## Steps to Reproduce

1. Start the emulator and seed/observe a few synthesis tasks so the store is non-empty (the suite's `support::seed_task` path).
2. Compute a token that was never minted: the hex encoding of the three-field string `0\u{1f}\u{1f}` (i.e. `hex::encode("0\u{1f}\u{1f}")`).
3. Call `GET /v1/synthesisTasks?NextToken=<that hex>` with no `Status` query parameter.
4. Observe HTTP 200 and a page of tasks, instead of the HTTP 400 `InvalidNextTokenException` the handler comment at list_speech_synthesis_tasks.rs:232-238 and the test rule at tests/all/synthesis_tasks.rs:286-288 state for a token this service did not mint.
5. Repeat with `hex::encode("99999999999999\u{1f}z\u{1f}")`: HTTP 200 with an empty `SynthesisTasks` list, again rather than a refusal.
