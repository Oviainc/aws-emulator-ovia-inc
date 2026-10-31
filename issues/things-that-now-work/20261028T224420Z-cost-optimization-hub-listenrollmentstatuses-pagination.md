Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 59eb119668c46536c53893c00cd680650c32abdf

# A nextToken carrying an offset the service never issued is served as an empty final page instead of being refused

_Filed: 2026-10-28T22:44:20Z_
_Operation: ListEnrollmentStatuses | Class: pagination_

> _Note: this issue was identified against the code base at commit `59eb119668c46536c53893c00cd680650c32abdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination token helpers validate a token's version marker and its list `kind` but never its OFFSET, and `paginate` then CLAMPS any offset to the collection length. A well-formed token the service never issued — e.g. hex of `cohv1:enrollment-statuses:4000` — is therefore accepted and answered HTTP 200 with `items: []` and no `nextToken`, which is byte-for-byte the answer of a completed listing. That contradicts the rule each paginated handler of this crate states for itself ("Refuse a `nextToken` this service did not issue for this list"), and it turns an invalid request into a success a client reads as "the listing ended".

The offset is decidable: `paginate` issues a token ONLY when rows remain after the page (`end >= all.len()` yields `None`), so every token this service ever hands out carries an offset strictly inside the collection. An offset at or past the end was never issued. No comment, test or doc in the crate justifies the clamp.

The same two helpers are the ones `ListRecommendations` and `ListRecommendationSummaries` page through, so all three listings share the hole; the crate's existing refusal tests only cover tokens that fail the version/kind check (`"not-a-token"`, `""`, `"6e6f7065"`).

## Evidence

src/ops/list_enrollment_statuses.rs:239-240 (the rule the handler states):
```
/// Refuse a `nextToken` this service did not issue for this list.
fn invalid_next_token() -> ListEnrollmentStatusesError {
```

src/ops/list_enrollment_statuses.rs:173-179 (the only check made):
```
        let offset = match &input.next_token {
            None => 0,
            Some(token) => match crate::state::decode_page_token(PAGE_KIND, token) {
                Some(offset) => offset,
                None => return Err(invalid_next_token()),
            },
        };
```

src/state.rs:853-860 (version and kind checked, offset unbounded):
```
/// The offset a `nextToken` resumes at, or `None` when the token was not issued
/// by [`encode_page_token`] for this `kind`.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let offset = text.strip_prefix(&format!("{PAGE_TOKEN_VERSION}:{kind}:"))?;
    offset.parse::<usize>().ok()
}
```

src/state.rs:880-889 (the clamp, and the proof that an out-of-range offset was never issued):
```
    let start = offset.min(all.len());
    ...
    let next_token = if items.is_empty() || end >= all.len() {
        None
    } else {
        Some(encode_page_token(kind, end))
    };
```

Existing coverage, which stops at the version/kind check: tests/all/cost_optimization_hub.rs:455-469 `for token in ["not-a-token", "", "6e6f7065"]` expects HTTP 400; no case sends a token with a well-formed prefix and an offset past the end.

## Steps to Reproduce

1. Hex-encode the string `cohv1:enrollment-statuses:4000` (the exact format `encode_page_token` emits, with an offset the service never issued).
2. Call ListEnrollmentStatuses with `{"includeOrganizationInfo": false, "nextToken": "<that hex>"}`.
3. Observe HTTP 200 with `items: []` and no `nextToken`, instead of the 400 ValidationException the handler raises for "a nextToken this service did not issue for this list" (list_enrollment_statuses.rs:239).
4. Repeat with `cohv1:recommendations:4000` on ListRecommendations and `cohv1:recommendation-summaries:4000` on ListRecommendationSummaries — same empty-200 answer, since all three share `decode_page_token`/`paginate`.
