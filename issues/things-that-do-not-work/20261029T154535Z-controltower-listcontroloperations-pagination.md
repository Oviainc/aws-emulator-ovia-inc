Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 56bafedc4ab2726cd06f6fbddd06102b2526f7ce

# A nextToken is a bare index that carries nothing about the listing it indexes, so a token minted under one filter resumes a different filter's listing and silently skips its earlier records

_Filed: 2026-10-29T15:45:35Z_
_Operation: ListControlOperations | Class: pagination_

> _Note: this issue was identified against the code base at commit `56bafedc4ab2726cd06f6fbddd06102b2526f7ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every list operation of this crate cuts its page through the one shared helper `crate::state::page`, and the four that narrow their listing from the request (`ListControlOperations`, `ListEnabledBaselines`, `ListEnabledControls`, `ListLandingZoneOperations`) pass `TokenScope::SameParameters` to say so. `TokenScope::SameParameters` is documented as "its token is only meaningful to a call that sent the same parameters", and the refusal the helper builds for a token it cannot resume asserts that same rule back to the caller ("was not returned by a previous {operation} call with these parameters"). Nothing implements it.

The token the helper mints is `end.to_string()` — a bare decimal index into whatever slice the caller handed it — and the only test it is put to on the way back in is `index <= len`. The token therefore carries nothing about the filter (or, for `ListEnabledControls`, the `targetIdentifier`) the listing it was minted from was narrowed by, and the helper has no way to tell one listing's position from another's. A caller that keeps a `nextToken` and changes (or drops) its filter is answered 200 with a page cut out of a DIFFERENT collection at that index: the records before the index in the new listing are never served to that caller, and the records it is served are ones the first page of the new listing would have returned. The refusal the handler's own comment and the helper's own message promise for exactly this case never fires — the error is turned into a success.

The same defect reaches `ListEnabledBaselines`, `ListEnabledControls` and `ListLandingZoneOperations`, which all declare `TokenScope::SameParameters` through the same helper; `ListControlOperations` is named here as the first of them on the operation list. It does NOT reach `ListBaselines`/`ListLandingZones`, which declare `TokenScope::WholeListing` and so make no such promise.

## Evidence

The scope the handler declares states the rule — `src/state.rs:3896-3898`:
```
    /// The operation narrows its listing from the request, so its token is only meaningful
    /// to a call that sent the same parameters.
    SameParameters,
```

The handler under review repeats it at the call site — `src/ops/list_control_operations.rs:227-234`:
```
        // The page the call asks for, cut out by this crate's one pagination rule: the
        // token is the index of the next unreturned record, so it is only meaningful
        // against a call carrying the same filter, which is what `nextToken` is documented
        // to require. `maxResults` is modelled `range 1..100`, so the front door has
        // already refused anything outside it.
        let (page, next_token) = crate::state::page(
            Self::NAME,
            crate::state::TokenScope::SameParameters,
```

The refusal asserts the rule to the caller — `src/state.rs:3946-3949`:
```
        TokenScope::SameParameters => format!(
            "nextToken {token} was not returned by a previous {operation} call with these \
             parameters"
        ),
```

But the helper only ever range-checks the index, and mints a bare index — `src/state.rs:3922-3932`:
```
    let len = listing.len();
    let start = match next_token {
        None | Some("") => 0usize,
        Some(token) => match token.parse::<usize>() {
            Ok(index) if index <= len => index,
            _ => return Err(unhandled_next_token_message(operation, scope, token)),
        },
    };
    let limit = max_results.map_or(len, |max| max.max(1) as usize);
    let end = start.saturating_add(limit).min(len);
    Ok((&listing[start..end], (end < len).then(|| end.to_string())))
```
`scope` is consumed only by `unhandled_next_token_message`; it takes no part in deciding whether the token is resumable.

The other three `SameParameters` call sites, same helper: `src/ops/list_enabled_baselines.rs:266-272`, `src/ops/list_enabled_controls.rs:250-256`, `src/ops/list_landing_zone_operations.rs:229-235`.

Test coverage: `tests/all/list_pagination.rs` drives all six listings through one table, and `token_scoped_to_parameters` is used ONLY to pick the refusal wording for an out-of-range/garbage token — `tests/all/list_pagination.rs:382-390`:
```
        let expected = format!(
            "nextToken {token} was not returned by a previous {} call{}",
            listing.operation,
            if listing.token_scoped_to_parameters {
                " with these parameters"
            } else {
                ""
            }
        );
```
The refused tokens in that table are `"a fractional index"`, `format!("an index past the {total} records listed")` and a baseline ARN (`:360-368`). No test in `tests/all/` carries a token from one narrowed listing into a call with different parameters, so nothing pins the current reading.

## Steps to Reproduce

1. Start the emulator and POST /enable-control six times for six distinct elective controls on organizational unit A, then six more on organizational unit B — twelve ENABLE_CONTROL records now sit in the control operation log.
2. POST /list-control-operations with {"filter": {"targetIdentifiers": ["<OU-A ARN>"]}, "maxResults": 5}. Six records match; the answer carries five summaries and nextToken "5".
3. POST /list-control-operations again with NO filter and {"nextToken": "5", "maxResults": 5}.
4. Observe 200 with the 6th..10th records of the TWELVE-record unfiltered listing: 5 <= 12, so `page` accepts a token that was minted as a position in the six-record OU-A listing. The first five records of the unfiltered listing are never served to this caller, and no record of OU-B's listing is skipped by any page the caller actually asked for.
5. Observe that the refusal the helper's own message and the handler's own comment promise for a token "not returned by a previous ListControlOperations call with these parameters" is never produced.
6. Repeat with /list-enabled-baselines (filter.targetIdentifiers), /list-enabled-controls (targetIdentifier) and /list-landingzone-operations (filter.types): each accepts the other listing's index through the same helper.
