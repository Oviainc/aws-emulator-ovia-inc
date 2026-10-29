Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 5f9b2775c0e43c5f90420530053d45f1b2563a83

# decode_page_token honours nextToken hex spellings encode_page_token never mints (uppercase/mixed-case), so a token this service never issued is served as a cursor

_Filed: 2026-10-29T16:05:25Z_
_Operation: ListSubscriptions | Class: pagination_

> _Note: this issue was identified against the code base at commit `5f9b2775c0e43c5f90420530053d45f1b2563a83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shared cursor primitives in `crate::state` are asymmetric about the hex alphabet they accept.

`encode_page_token` mints a token with `hex::encode`, which emits LOWERCASE hex only, so every `nextToken` this service has ever issued is lowercase. `decode_page_token` reads it back with `hex::decode`, which — as the `hex` crate documents — accepts uppercase and mixed-case input as well ("Both uppercase and lowercase characters are valid in the input string and can even be mixed"). An uppercased or mixed-case re-spelling of an issued token therefore decodes to the same plaintext, so the scope matches and `page_token_tag` recomputes the same tag, and the token is accepted as a resume point.

That contradicts the rule the same module states for itself twice: `decode_page_token`'s own doc says it answers `None` "when the token is not one this service issued FOR THAT collection — a fabricated token, a token whose tag does not match the key it carries, and a token another listing minted all answer `None`", and `Page`'s doc says "a `nextToken` is a resume point only when this service issued it FOR THIS collection". A spelling the minting side cannot produce is, by that rule, a value the decode side must refuse; instead `Page::resolve` hands back a page and the request that should have earned the crate's own `UNISSUED_PAGE_TOKEN_DETAIL` refusal (400) is answered 200.

`ListSubscriptions` is the caller named here (its handler passes `input.next_token` straight into `Page::resolve`); `ListDomains` reaches the identical decode through `domains_page_scope`, so one fix in `decode_page_token` covers both listings.

## Evidence

The minting side — lowercase only — services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1516-1519:
```
pub fn encode_page_token(scope: &str, last_key: &str) -> String {
    let tag = page_token_tag(scope, last_key);
    hex::encode(format!("{tag}:{scope}:{last_key}").as_bytes())
}
```

The reading side, with the rule it claims to enforce — src/state.rs:1521-1535:
```
/// The key a `nextToken` resumes after in `scope`, or `None` when the token is not one
/// this service issued FOR THAT collection — a fabricated token, a token whose tag does
/// not match the key it carries, and a token another listing minted all answer `None`.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    if token.len() > MAX_PAGE_TOKEN_LEN {
        return None;
    }
    let plaintext = String::from_utf8(hex::decode(token).ok()?).ok()?;
```
`hex::decode` (the `hex` crate, Cargo.lock:3177 `name = "hex"`) is documented as case-insensitive: "Both uppercase and lowercase characters are valid in the input string and can even be mixed (e.g. f9b4ca, F9B4CA and f9B4Ca are all valid strings)." The tag is recomputed over the DECODED plaintext (src/state.rs:1533, `page_token_tag(scope, after_key) == tag`), so re-casing the hex changes neither the scope nor the tag and the token verifies.

The policy the same module states — src/state.rs:1561-1566:
```
/// This is the pagination POLICY both listings of this service obey, held once beside the
/// cursor primitives it is built on ([`encode_page_token`] / [`decode_page_token`] and
/// the `*_page_scope` constructors): absent `maxResults` means a full page and a value
/// outside the bound is refused rather than clamped; a `nextToken` is a resume point only
/// when this service issued it FOR THIS collection; and a page issues a cursor only when
/// a matching record is left after it.
```

The refusal that should have been answered — src/state.rs:1554-1556 and :1608-1613:
```
const UNISSUED_PAGE_TOKEN_DETAIL: &str = "nextToken is not a token this service issued";
```
```
            Some(token) => match decode_page_token(&scope, token) {
                Some(key) => Some(key),
                None => return Err(UNISSUED_PAGE_TOKEN_DETAIL.to_string()),
```

The caller — src/ops/list_subscriptions.rs:161-175 (`ListDomains` reaches the same decode at src/ops/list_domains.rs:202-209):
```
        let page = match crate::state::Page::resolve(
            crate::state::subscriptions_page_scope(&region, &input.domain_id),
            input.max_results,
            input.next_token.as_deref(),
        ) {
```

Test coverage: tests/all/shared_pagination_policy.rs:283 `async fn both_listings_resume_only_from_a_cursor_they_issued()` walks the refusing half over exactly these rows (tests/all/shared_pagination_policy.rs:375-381):
```
        let rows = [
            ("hex of invented text", hex::encode("dom-not-a-cursor")),
            ("not hex at all", "zz-not-a-token".to_string()),
            ("empty", String::new()),
            ("a cursor the other listing issued", other),
            ("far longer than any cursor", "61".repeat(600)),
        ];
```
— no row re-cases an issued token, so the accepted spelling is unpinned.

## Steps to Reproduce

1. POST /domain with {"name":"clinic"} and capture domainId; POST /domains/{domainId}/subscriptions three times.
2. GET /domains/{domainId}/subscriptions?maxResults=1 and capture the issued nextToken — every character of it is lowercase hex, because src/state.rs:1518 mints it with hex::encode.
3. Uppercase the token's letters (e.g. 'a3f1…' -> 'A3F1…'). This spelling is one no code path in this crate can produce.
4. GET /domains/{domainId}/subscriptions?maxResults=1&nextToken=<uppercased token>.
5. Expected per src/state.rs:1521-1523 and :1561-1566: 400 ValidationException with "nextToken is not a token this service issued". Actual: 200, resuming the page — hex::decode accepted the re-cased input and the tag recomputed over the identical plaintext matched.
6. Repeat against GET /domain?maxResults=1 to see the same acceptance on ListDomains, which shares decode_page_token.
