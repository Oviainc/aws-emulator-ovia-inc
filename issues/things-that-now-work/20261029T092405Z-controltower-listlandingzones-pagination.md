Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: feb204971e0c529b59922c31c571e9b68abdb6f7

# Page tokens are honoured in spellings the one minting site never writes ("+N" and leading zeros), so a token the service never issued silently skips records

_Filed: 2026-10-29T09:24:05Z_
_Operation: ListLandingZones | Class: pagination_

> _Note: this issue was identified against the code base at commit `feb204971e0c529b59922c31c571e9b68abdb6f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every listing of this crate (`ListLandingZones`, `ListLandingZoneOperations`, `ListControlOperations`, `ListEnabledControls`, `ListEnabledBaselines`, `ListBaselines`) reaches one shared pagination rule, `crate::state::page`. That rule MINTS a token with `end.to_string()` — always a canonical decimal with no sign and no leading zeros — but ACCEPTS a token with `token.parse::<usize>()`, and Rust's integer `FromStr` accepts a leading `+` as well as any number of leading zeros. So `"+2"`, `"02"`, `"0002"` and `"+0002"` all parse to 2 and are served as the page starting at index 2, even though no call of this service ever handed any of those strings out.

The consequence is a silent data loss on a read: a caller (or a buggy client that re-pads or re-signs a token) that sends `"02"` against a listing of four enabled controls is answered 200 with the LAST TWO records, with the first two silently missing and nothing in the response saying a page was skipped — instead of the `ValidationException` (400) the crate's own refusal path exists to give. The same handler refuses `"-1"`, `"1.5"` and `"not-a-token"`, so the acceptance of `"+2"`/`"02"` is not a deliberate lenient parse but a side effect of `parse::<usize>()` being used as the membership test for "a token a previous page returned".

The defect is in the single shared helper, so it is the same on all six listings; `ListLandingZones` is named here as the operation whose handler calls it (and the operation the crate's own pagination-rule work was centred on).

## Evidence

The MINTING site only ever writes a canonical decimal — `services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:3737-3747`:
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
`end.to_string()` never produces `"+2"` or `"02"`, yet `"+2".parse::<usize>()` and `"02".parse::<usize>()` both yield `Ok(2)`, so both are taken as page positions.

The rule the same helper's own doc states — `src/state.rs:3720-3725`:
```
/// The ONE place every list operation of this service cuts a page out of its listing, so a
/// page boundary cannot drift between them. The rule: an absent or empty token starts at
/// the beginning, any other token is the index of the first entry the previous page did
/// not return and resumes there — and is refused when it is not an index into this
/// listing;
```
and the wording the refusal is built with — `src/state.rs:3756-3765`:
```
fn unhandled_next_token_message(operation: &str, scope: TokenScope, token: &str) -> String {
    match scope {
        TokenScope::WholeListing => {
            format!("nextToken {token} was not returned by a previous {operation} call")
        }
```
The refusal's own criterion is "was not returned by a previous call", which `"+2"` and `"02"` satisfy while still being accepted.

The crate states the same rule in its tests — `tests/all/landing_zone_operations.rs:643-648`:
```
    // A token this operation never handed out is refused, not silently read as the first
    // page — the same rule this service's other listings apply to their own tokens.
    for (case, token) in [
        ("a token that is not a number at all", "not-a-token"),
        ("a token past the end of the listing", "99"),
    ] {
```

Test coverage today, `tests/all/list_pagination.rs:353-369` (`an_unhandled_token_is_refused`), drives exactly five token shapes over all six listings — `"not-a-token"`, `"-1"`, `"1.5"`, `(total + 1)` and a baseline ARN — and no `"+N"` or zero-padded spelling, which is why the gap is unnoticed:
```
    let cases: Vec<(String, String)> = vec![
        (
            "a token that is not a number".to_string(),
            "not-a-token".to_string(),
        ),
        ("a negative index".to_string(), "-1".to_string()),
        ("a fractional index".to_string(), "1.5".to_string()),
```

## Steps to Reproduce

1. Start the emulator and seed records a listing pages over — e.g. POST /create-landingzone with a well-formed manifest, then POST /enable-control four times for four distinct control/target pairs (as tests/all/list_pagination.rs:410-492 seeds them), and wait past the settle span.
2. POST /list-enabled-controls with `{}` and note the four `enabledControls` entries in ARN order; no `nextToken` is reported.
3. POST /list-enabled-controls with `{"nextToken": "02"}` — a string no page of this service ever returned (the handler only ever writes `end.to_string()`, i.e. `"2"`). Observed: HTTP 200 with the listing resumed at index 2, so the first two enabled controls are silently missing. Expected: HTTP 400 ValidationException "nextToken 02 was not returned by a previous ListEnabledControls call with these parameters", the same refusal `"1.5"` and `"-1"` already get.
4. Repeat with `{"nextToken": "+2"}` and `{"nextToken": "0002"}` — both are likewise answered 200 from index 2.
5. Repeat against /list-landingzones with `{"nextToken": "+1"}` (one landing zone stored): answered 200 with an empty, untokened page instead of the modelled ValidationException, so a caller cannot tell "end of listing" from "token this service never issued".
