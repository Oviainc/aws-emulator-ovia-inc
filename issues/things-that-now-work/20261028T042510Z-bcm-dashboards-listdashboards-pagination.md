Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 21d30b53374928fd663edcc8ae486344ab510a4e

# decode_page_token accepts a nextToken this service never minted, against the pattern check its own doc states

_Filed: 2026-10-28T04:25:10Z_
_Operation: ListDashboards | Class: pagination_

> _Note: this issue was identified against the code base at commit `21d30b53374928fd663edcc8ae486344ab510a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::decode_page_token` is the one cursor decoder both listings use. Its doc says the decoded value is checked against the fixed segments of the resource's ARN pattern, naming `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:<resource>/` explicitly, so that a token which decodes to anything else is REFUSED rather than read as a resume point. The code under that doc performs three unanchored checks instead — `starts_with("arn:aws")`, `contains(":bcm-dashboards::")`, `contains(arn_segment)` — and never checks the 12-digit account segment, nor that the segments appear in the pattern's order/positions. A hand-written token whose body is the hex of a string that satisfies those three loose checks but not the quoted pattern (e.g. `arn:aws:bcm-dashboards::nope:dashboard/zz`) is therefore accepted as a resume point: `listing_page` uses it as `Bound::Included` and the call answers HTTP 200 with a silently truncated or empty page (and no `nextToken`) instead of the `ValidationException` the listing raises for `ListingRefusal::UninterpretableToken`. Same defect through `ListScheduledReports`, which shares the helper.

## Evidence

Doc side, `src/state.rs:1580-1589`: "The ARN a `nextToken` resumes at, or `None` when the token is not one this service minted for a listing over `arn_segment`'s resource type (the caller answers that with a `ValidationException`). A cursor this service minted always names an ARN of the resource the listing walks. It is checked against the fixed segments of that resource's ARN pattern (`^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:<resource>/...`), so a token that decodes to anything else — including a cursor minted by a listing over the other resource type — is rejected rather than read as a resume point past every stored record."

Code side, `src/state.rs:1590-1598`:
```
pub fn decode_page_token(token: &str, arn_segment: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let bytes = hex::decode(body).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    let names_the_resource = arn.starts_with("arn:aws")
        && arn.contains(":bcm-dashboards::")
        && arn.contains(arn_segment);
    names_the_resource.then_some(arn)
}
```
There is no `[0-9]{12}` account check and no positional anchoring, so the quoted pattern is not what is applied.

Consumer, `src/state.rs:1659-1674`: `decode_page_token(token, arn_segment).ok_or(ListingRefusal::UninterpretableToken)?` then `Bound::Included(arn)`, reached from `src/ops/list_dashboards.rs:168-179` (and `src/ops/list_scheduled_reports.rs:171-182`).

Test coverage: `tests/all/listing_pagination.rs:289-313` offers only tokens that fail the prefix/hex/segment checks (`"not-a-token"`, `"v1:"`, `"v1:zzzz"`, `"v1:6e6f"`, `"deadbeef"`, `"{\"page\": 2}"`, the other listing's cursor) — no row carries a token that passes the three loose checks while failing the documented pattern, so this path is uncovered.

## Steps to Reproduce

1. Start the service (a fresh store already holds the AWS-managed dashboard catalog, so ListDashboards has records).
2. Hex-encode the ASCII string `arn:aws:bcm-dashboards::nope:dashboard/zz` and prefix the result with `v1:` to build a token this service never minted.
3. Call ListDashboards with that value as `nextToken` — expect ValidationException (400) "Member must be a nextToken returned by a previous ListDashboards call: nextToken"; observe HTTP 200 with an empty `dashboards` list and no `nextToken`.
4. Repeat with `arn:aws:bcm-dashboards::nope:scheduled-report/zz` against ListScheduledReports for the same result.
