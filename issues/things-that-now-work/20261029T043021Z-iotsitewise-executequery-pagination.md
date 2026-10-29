Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec

# ExecuteQuery honours a nextToken it never issued: a bare row offset bound to no statement, and an offset at the end of the result answered as an empty page

_Filed: 2026-10-29T04:30:21Z_
_Operation: ExecuteQuery | Class: pagination_

> _Note: this issue was identified against the code base at commit `96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ExecuteQuery's `nextToken` is nothing but a hex-encoded row offset. The handler decodes it and then applies ONE sanity check — `start > result.rows.len()` — before slicing the rows of the statement THIS request carries. Two consequences, both contradicted by this crate's own siblings:

1. The token is bound to nothing that identifies the enumeration it came from. A token minted for statement A (e.g. offset 1) replayed against a DIFFERENT statement B is accepted, and B's result is served from row 1 onwards: B's first row appears in no page of B's enumeration and the client is never told. The token FORM is shared with GetQueryResults through `crate::ops::execute_query::encode_row_offset` / `decode_row_offset`, so a token minted by GetQueryResults for some workspace query's rows is equally honoured here. The crate's third batch/paged reader does exactly the missing check: BatchGetAssetPropertyValue carries a FINGERPRINT of the request beside the position so "a token is only honoured against the request that produced it", and a test pins the refusal.

2. An offset EQUAL to the row count is accepted (`start > len` instead of `start >= len`), so ExecuteQuery answers 200 with zero rows and no token for a token it provably never issued — it mints one only while `end < result.rows.len()`. That is the "report absence as success" answer the crate's shared page walk explicitly forbids, and the sibling GetQueryResults — the other operation that pages the SAME engine's rows, through the same token codec — refuses that exact case, with a test row for it.

Both symptoms are the same defect: the refusal message claims "The nextToken is not a token this operation issued" while the check cannot tell such a token apart.

## Evidence

The whole validation of the token, services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/execute_query.rs:183-200:
```
        // Paging over the computed result: `nextToken` is the index of the first row
        // of the next page, hex-encoded (the member carries no pattern, and hex keeps
        // it to the same alphabet every other token this service issues uses).
        let start = match input.next_token.as_deref() {
            Some(token) => decode_token(token)?,
            None => 0,
        };
        if start > result.rows.len() {
            return Err(invalid_request(
                "The nextToken is not a token this operation issued.".to_string(),
            ));
        }
        ...
        let next_token = (end < result.rows.len()).then(|| encode_row_offset(end));
```
and the codec it decodes with, src/ops/execute_query.rs:2050-2062 — "pub fn decode_row_offset(token: &str) -> Option<usize> { ::hex::decode(token).ok().and_then(|bytes| String::from_utf8(bytes).ok()).and_then(|text| text.parse().ok()) }" ... "fn decode_token(token: &str) -> Result<usize, ExecuteQueryError> { decode_row_offset(token).ok_or_else(|| invalid_request(\"The nextToken is not a token this operation issued.\".to_string())) }" — i.e. ANY hex of a number is accepted, and src/ops/execute_query.rs:2033-2041 states the form is shared: "The token FORM belongs to this engine — the index of the first row of the next page, hex-encoded so it stays opaque to the caller — and every operation that pages a query's rows issues that same form".

The sibling that binds its page token to the request that produced it, src/ops/batch_get_asset_property_value.rs:386-401:
```
/// The token carries the FINGERPRINT of the entry list beside the position, so a
/// token is only honoured against the request that produced it: replayed against a
/// different list it names no page, rather than silently re-paging that other list
/// and reporting its leading entries as already processed.
...
fn decode_page_token(token: &str, entries: &[BatchGetAssetPropertyValueEntry]) -> Option<usize> {
    ...
    (fingerprint == entries_fingerprint(entries)).then_some(())?;
```

The sibling that pages the SAME engine's rows and refuses an offset at the end, src/ops/get_query_results.rs:235-255:
```
        // `nextToken` is the index of the first row of the next page, in the one
        // opaque form the engine that issues it owns ([`encode_row_offset`] /
        // [`decode_row_offset`]), so the two operations that page a query's rows
        // cannot drift about the form they emit or the text they read back. WHICH
        // offsets this operation accepts is its own question, answered below: a
        // token is issued only for a row this result HAS, so an offset at or past
        // the end is one it never issued ...
                if resume >= held {
                    return Err(invalid_token());
                }
```

The crate-wide rule the lenient check breaks, src/state.rs:6689-6692 (the shared page walk used by all 34 listings) — "`None` for a `next_token` this service never issued — text that is not the hex of a resume key, or a key no entry of this enumeration carries. Its caller answers that with the modelled refusal for an invalid `nextToken`, rather than with an empty page, which would report absence as success."

Test coverage: nothing pins the lenient behaviour. tests/all/execute_query.rs:394-431 pages ONE statement with its own token ("the pages must walk every row exactly once") and never replays a token against another statement or an at-the-end offset. The refusals the siblings DO pin: tests/all/batch_get_latest_values.rs:655-675 ("A page token belongs to the request that produced it: replayed against a different entry list it names no page" -> 400) and tests/all/query_results.rs:586-609, whose rows include `("an offset at the end of the result", at_the_end)` and expect 400 InvalidRequestException.

## Steps to Reproduce

1. Create an asset model and two assets of it (so `SELECT asset_name FROM asset` answers at least two rows), through CreateAssetModel/CreateAsset.
2. POST /queries/execution with {"queryStatement": "SELECT asset_name FROM asset ORDER BY asset_name", "maxResults": 1} -> 200 with one row and a nextToken (the hex of "1").
3. POST /queries/execution again with a DIFFERENT statement — e.g. {"queryStatement": "SELECT asset_id FROM asset ORDER BY asset_id", "maxResults": 10} — carrying that same nextToken.
4. Observed: 200, and the answer starts at row index 1 of the second statement's result; that statement's first row is reported in no page of this enumeration. Expected, per the rule the sibling batch reader states and a test pins (src/ops/batch_get_asset_property_value.rs:386-389, tests/all/batch_get_latest_values.rs:655-675): a token replayed against a different query names no page and is refused with InvalidRequestException.
5. Second symptom: run a statement that answers exactly N rows with no maxResults (one page, no nextToken is issued), then POST the same statement with nextToken = hex of "N" (e.g. N=2 -> "32").
6. Observed: 200 with an empty rows list and no nextToken, for a token ExecuteQuery never mints (src/ops/execute_query.rs:200 mints one only while `end < rows.len()`). Expected: 400 InvalidRequestException, exactly as GetQueryResults answers the same at-the-end offset (src/ops/get_query_results.rs:246-251, pinned by tests/all/query_results.rs:595).
