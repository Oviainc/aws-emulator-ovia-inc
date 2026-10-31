Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f215f769af13799ee4e193f9a237039caca26230

# decode_page_token honours uppercase/mixed-case hex cursor bodies that encode_page_token never mints, so a nextToken this service never issued is served as a resume point

_Filed: 2026-10-29T14:18:32Z_
_Operation: ListScheduledReports | Class: pagination_

> _Note: this issue was identified against the code base at commit `f215f769af13799ee4e193f9a237039caca26230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both listings share one cursor codec in `crate::state`. `encode_page_token` renders the resume ARN with `hex::encode`, which emits LOWERCASE hex only, so every cursor this service hands out has a lowercase body. `decode_page_token` reads the body back with `hex::decode`, which accepts upper-case and mixed-case hex digits as well (the `hex` crate documents "Both upper and lower case characters are valid in the input string and can even be mixed"). An uppercase (or mixed-case) spelling of the hex body decodes to exactly the same ARN bytes, so it passes `names_resource_arn` and is used as `Bound::Included(arn)` — a token this service never minted is accepted as a resume point and answered HTTP 200 with a page resumed from a hand-written cursor, instead of the `ValidationException` the listing raises for `ListingRefusal::UninterpretableToken`.

That contradicts the rule the crate states for its own cursor format in two places: the prefix exists precisely "so a token this service did not mint is rejected instead of being read as a resume point", and the `ListScheduledReports` handler doc promises that "A `nextToken` this service did not mint for a scheduled-report listing is refused with `ValidationException` (400)". `ListDashboards` is affected identically through the same helper.

## Evidence

The rule the crate states — `src/state.rs:114-116`:
```
/// The prefix a pagination cursor carries, so a token this service did not mint is
/// rejected instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1:";
```
The handler doc that promises the refusal — `src/ops/list_scheduled_reports.rs:9-11`:
```
//! `nextToken` this service did not mint for a scheduled-report listing is refused with
//! `ValidationException` (400);
```
What is minted (lowercase only) — `src/state.rs:1999-2001`:
```
pub fn encode_page_token(arn: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(arn))
}
```
What is accepted (any hex case) — `src/state.rs:2079-2084`:
```
pub fn decode_page_token(token: &str, resource_type: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let bytes = hex::decode(body).ok()?;
    let arn = String::from_utf8(bytes).ok()?;
    names_resource_arn(&arn, resource_type).then_some(arn)
}
```
The decoded ARN is identical for either spelling, so the pattern check added for the earlier cursor issue does not reject it, and it reaches `Bound::Included(arn)` in `listing_page` (`src/state.rs:2156-2160`), reached from `src/ops/list_scheduled_reports.rs:171-177`.

Test coverage: the uninterpretable-token table only offers bodies that fail the prefix/hex/pattern checks — `tests/all/listing_pagination.rs:296-304` rows `"not-a-token"`, `"v1:"`, `"v1:zzzz"`, `"v1:6e6f"`, `"deadbeef"`, `"{\"page\": 2}"`, the other listing's cursor (same rows at `tests/all/scheduled_report_listing.rs:494-496` and `tests/all/dashboard_lifecycle.rs:796-798`). No row carries a case-variant of a real cursor, so this path is uncovered and nothing pins acceptance.

## Steps to Reproduce

1. Create a dashboard, then create three scheduled reports on it.
2. Call ListScheduledReports with maxResults=1 and keep the answered nextToken (it looks like `v1:61726e...`).
3. Upper-case every hex digit after the `v1:` prefix — a token `encode_page_token` can never produce.
4. Call ListScheduledReports again with that value as nextToken: expect `ValidationException` (400) "Member must be a nextToken returned by a previous ListScheduledReports call: nextToken"; observe HTTP 200 with the walk resumed as if the service had minted the token.
5. Repeat against ListDashboards with an upper-cased dashboard cursor for the same result.
