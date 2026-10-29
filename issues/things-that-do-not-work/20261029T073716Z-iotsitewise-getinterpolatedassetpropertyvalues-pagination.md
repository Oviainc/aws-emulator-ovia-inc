Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# The nextToken is a bare interval ordinal bound to no request, so a token minted for one property/window is honoured against another and silently skips intervals

_Filed: 2026-10-29T07:37:16Z_
_Operation: GetInterpolatedAssetPropertyValues | Class: pagination_

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This operation's `nextToken` carries only the ordinal of the interval a page resumes at (`hex(resume.to_string())`). Nothing in it identifies the request it was minted for: not the stream (`assetId`+`propertyId` or `propertyAlias`), not `startTimeInSeconds`/`startTimeOffsetInNanos`, not `intervalInSeconds`, not `type`, not `quality`. The only check on replay is that the ordinal is in range for THIS request's grid (`resume < 1 || resume > last_index`). So a token issued for property A's 1-hour grid, replayed with property B, or with a different start/interval/type/quality, is accepted as a position on the new grid: the caller is served the page starting at interval N of an enumeration that token never described, and intervals 1..N of it are silently skipped (reported to the caller as already delivered) instead of the "not a token this operation issued" refusal the handler's own helper exists for.

The crate already has the rule that fixes it: the sibling batch reader binds the token to the request with a fingerprint, so "a token is only honoured against the request that produced it".

## Evidence

The unbound token — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_interpolated_asset_property_values.rs:413-427:
```
        // `nextToken` is this enumeration's resume point: the ordinal of the interval
        // the next page starts at, hex-encoded so it matches the modelled
        // `^[A-Za-z0-9+/=]+$`. A token that names no interval of this range is refused
        // rather than answered with an empty page; the first page's own ordinal (0) is
        // not one this operation ever issues.
        let mut cursor = match input.next_token.as_deref() {
            Some(token) => {
                let resume = decode_token(token)?;
                if resume < 1 || resume > last_index {
                    return Err(invalid_token());
                }
                resume
            }
            None => 0,
        };
```
and what the token actually holds — same file, lines 642-655:
```
/// A `nextToken` for the interval the next page resumes from: its ordinal, written
/// out and hex-encoded so the token matches the modelled `^[A-Za-z0-9+/=]+$`.
fn encode_token(resume: u64) -> String {
    ::hex::encode(resume.to_string().as_bytes())
}

/// Decode a `nextToken` back into the interval ordinal it resumes from.
fn decode_token(token: &str) -> Result<u64, GetInterpolatedAssetPropertyValuesError> {
    ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|resume| resume.parse().ok())
        .ok_or_else(invalid_token)
}
```

The sibling rule in this same crate — src/ops/batch_get_asset_property_value.rs:382-401:
```
/// The `nextToken` for a page of `entries` resuming at entry `at`, in the hex form
/// this service's other paginated operations issue ...
///
/// The token carries the FINGERPRINT of the entry list beside the position, so a
/// token is only honoured against the request that produced it: replayed against a
/// different list it names no page, rather than silently re-paging that other list
/// and reporting its leading entries as already processed.
fn encode_page_token(at: usize, entries: &[BatchGetAssetPropertyValueEntry]) -> String {
    ::hex::encode(format!("{at}|{}", entries_fingerprint(entries)))
}
```
and the handler's own statement that a lost/foreign resume point must be refused rather than silently repositioned — src/ops/get_interpolated_asset_property_values.rs:657-659: "HTTP 400 `InvalidRequestException` for a `nextToken` this operation never issued."

Test coverage: tests/all/interpolated_property_values.rs:724-849 pins same-request paging and refuses malformed/out-of-range tokens (`"zz"`, hex("0"), hex("9999")); no case replays a token against a different property, window, interval, type or quality, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateAssetModel + CreateAsset with two DOUBLE measurement properties P1 and P2; BatchPutAssetPropertyValue readings covering the same window for both.
2. GET /properties/interpolated?assetId=A&propertyId=P1&startTimeInSeconds=S&endTimeInSeconds=E&intervalInSeconds=60&type=LINEAR_INTERPOLATION&quality=GOOD&maxResults=2 -> 200 with a nextToken (ordinal 2 for P1's grid).
3. Replay that same nextToken with propertyId=P2 (or with intervalInSeconds=3600, or type=LOCF_INTERPOLATION, or a different startTimeInSeconds) -> observed: 200, a page starting at interval 2 of the NEW enumeration, with intervals 1..1 of it never delivered to the caller.
4. Expected, by the crate's own rule at src/ops/batch_get_asset_property_value.rs:382-401 and this handler's invalid_token() helper: a token not minted for this request names no page and is refused with InvalidRequestException.
