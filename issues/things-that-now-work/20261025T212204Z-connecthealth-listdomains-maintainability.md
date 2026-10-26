Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# Deduplicate the page-size bound and cursor handling copied between ListDomains and ListSubscriptions

_Filed: 2026-10-25T21:22:04Z_
_Operation: ListDomains | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two paginated handlers of this crate implement the SAME pagination rule twice, in two files, with no shared helper:

1. the page-size bound and its constants (`MIN_PAGE_SIZE`/`MAX_PAGE_SIZE`, with the same doc paragraph in both files),
2. the "absent means a full page / out of range is REFUSED, never clamped" branch, worded through `crate::service::error_detail::range_violation_message`,
3. the "a `nextToken` is a resume point only when `decode_page_token` accepts it for THIS collection's scope, otherwise refuse" branch,
4. the "issue a token only when a matching record is left after this page" decision (`matching.next()` paired with `page.last()`).

Only the pagination PRIMITIVES are shared (`crate::state::encode_page_token` / `decode_page_token` / `*_page_scope`); the policy built on top of them is duplicated. The two copies differ only in which error channel the refusal travels on (`ListDomains` models no errors, so it uses `crate::scaffolded::Scaffolded::validation`, while `ListSubscriptions` has a modelled `ValidationException`), so the shared part can be factored into one helper that returns the resolved page size / resume key or the refusal TEXT, with each handler wrapping that text in its own channel. Nothing on the wire changes.

The risk is drift: the crate's own tests assert the two operations behave identically here, so a change applied to one copy and not the other silently splits one documented rule into two. This is the only maintainability issue filed for this service.

## Evidence

Copy 1 — services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_domains.rs:63-72:
```
/// The page-size bound this operation enforces itself, and the page it serves when the
/// caller asks for no `maxResults` (the maximum).
...
const MIN_PAGE_SIZE: i32 = 1;
const MAX_PAGE_SIZE: i32 = 100;
```
and list_domains.rs:172-188:
```
        let page_size = match input.max_results {
            None => MAX_PAGE_SIZE,
            Some(requested) if (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&requested) => requested,
            Some(requested) => {
                return Err(crate::scaffolded::Scaffolded::validation(
                    crate::service::error_detail::range_violation_message(
                        "maxResults", requested, MIN_PAGE_SIZE, MAX_PAGE_SIZE,
                    ),
                ))
```
and list_domains.rs:197-207:
```
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_page_token(&scope, token) {
                Some(key) => Some(key),
                None => {
                    return Err(crate::scaffolded::Scaffolded::validation(
                        "nextToken is not a token this service issued",
                    ))
```
and list_domains.rs:235-240:
```
        let next_token = match (matching.next(), domains.last()) {
            (Some(_), Some(last)) => {
                Some(crate::state::encode_page_token(&scope, &last.domain_id))
            }
            _ => None,
        };
```

Copy 2 — services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_subscriptions.rs:74-83 (identical constants and doc paragraph):
```
const MIN_PAGE_SIZE: i32 = 1;
const MAX_PAGE_SIZE: i32 = 100;
```
and list_subscriptions.rs:174-193:
```
        let page_size = match input.max_results {
            None => MAX_PAGE_SIZE,
            Some(requested) if (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&requested) => requested,
            Some(requested) => {
                return Err(ListSubscriptionsErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(crate::service::error_detail::range_violation_message(
                            "maxResults", requested, MIN_PAGE_SIZE, MAX_PAGE_SIZE,
                        )),
```
and list_subscriptions.rs:202-216 (same decode-or-refuse branch, same sentence "nextToken is not a token this service issued"), and list_subscriptions.rs:252-258 (same `matching.next()` / `last()` token decision).

Both copies are pinned as ONE rule by existing tests, which is exactly why they must not drift:
- services/service-com-amazonaws-connecthealth-connecthealth/tests/all/error_detail_member.rs:274: "// Both list operations bind `maxResults` the same way, so the rule covers both." — the same 7-row table (`min-1`,`-7`,`min`,`max`,`max+1`,`1_000`,`i32::MAX`) is asserted against `/domain` and `/domains/{id}/subscriptions`.
- services/service-com-amazonaws-connecthealth-connecthealth/tests/all/pagination_token_issuance.rs:6-14: "both list operations are `@paginated(...)` with an OPAQUE, server-issued token ... every other value ... is refused with a `ValidationException` (400)", asserted by `list_domains_resumes_only_from_a_cursor_it_issued` (line 152) and `list_subscriptions_resumes_only_from_a_cursor_it_issued` (line 271).

The fix must keep both responses byte-identical and both test suites passing unchanged.

## Steps to Reproduce

1. Open services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_domains.rs and services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_subscriptions.rs side by side.
2. Compare lines 63-72 with 74-83 (the duplicated MIN_PAGE_SIZE / MAX_PAGE_SIZE constants and their doc paragraph).
3. Compare lines 172-188 with 174-193 (the duplicated refuse-not-clamp page-size branch) and lines 197-207 with 202-216 (the duplicated decode-or-refuse cursor branch) and lines 235-240 with 252-258 (the duplicated next-token decision).
4. Observe that crate::state only shares the token primitives (encode_page_token / decode_page_token / *_page_scope); the policy around them exists twice, while tests/all/error_detail_member.rs:274-311 and tests/all/pagination_token_issuance.rs assert the two operations obey one identical rule.
