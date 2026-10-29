Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f215f769af13799ee4e193f9a237039caca26230

# decode_page_token honours NextToken hex spellings encode_page_token never mints (uppercase/mixed-case), so a token this service never issued is served as a cursor

_Filed: 2026-10-29T14:21:43Z_
_Operation: ListDomains | Class: pagination_

> _Note: this issue was identified against the code base at commit `f215f769af13799ee4e193f9a237039caca26230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of this service's five list operations takes its page through the shared helper `crate::state::page`, whose `NextToken` is minted by `encode_page_token` as `hex::encode("<collection>\u{1f}<fingerprint>\u{1f}<last key>")` and read back by `decode_page_token` with `hex::decode(token)`.

`hex::encode` (hex 0.4.3, Cargo.lock:3176-3180) writes LOWERCASE hex digits only, while `hex::decode` accepts upper-, lower- and mixed-case digits for the same bytes. So the uppercase (or any mixed-case) spelling of an issued token — a byte sequence `encode_page_token` can never produce, i.e. a token this service never handed out — decodes to exactly the same `<collection>\u{1f}<fingerprint>\u{1f}<last key>` plaintext, passes the collection and fingerprint equality checks, and is served HTTP 200 with the page that follows that cursor.

That contradicts the rule the crate states for itself in three places: the helper's own doc ("refusing one that was not issued for this collection and filter"), the `PageError::InvalidToken` doc ("A `NextToken` this collection and filter cannot resume from"), and the sentence a handler answers with — "nextToken is not a pagination token issued by a previous {operation} call with the same parameters". A token nobody issued is accepted as a cursor instead of being refused as the invalid input the crate says it is; a made-up token that is *not* a case variant (`deadbeef`) is refused, so the handler's answer to "was this token issued by me?" depends on nothing but the case of the hex digits.

The defect lives in the shared helper, so the same laxity applies to ListDomains, ListObjectives, ListCommonControls, ListControls and ListControlMappings; ListDomains is the plainest caller (no filter) and the one the crate's own token-replay test uses.

## Evidence

The token is minted lowercase-only — services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:645-650:
```
/// Encode an opaque `NextToken` for the given collection, filter and last-returned key.
fn encode_page_token(collection: &str, fingerprint: &str, last_key: &str) -> String {
    hex::encode(format!(
        "{collection}{TOKEN_SEPARATOR}{fingerprint}{TOKEN_SEPARATOR}{last_key}"
    ))
}
```
…and read back with a case-insensitive decoder — src/state.rs:652-660:
```
/// Decode a `NextToken` back to the key it resumes after, refusing one that was not
/// issued for this collection and filter.
fn decode_page_token(
    token: &str,
    collection: &str,
    fingerprint: &str,
) -> Result<String, PageError> {
    let decoded = hex::decode(token).map_err(|_| PageError::InvalidToken)?;
```
The crate's own statement of what must be refused — src/state.rs:609-611:
```
pub enum PageError {
    /// A `NextToken` this collection and filter cannot resume from.
    InvalidToken,
```
and the sentence it is refused with — src/state.rs:687-690:
```
        PageError::InvalidToken => format!(
            "nextToken is not a pagination token issued by a previous {operation} call \
             with the same parameters"
        ),
```
The handler that reaches both through `page` — src/ops/list_domains.rs:175-190:
```
        let page = crate::state::page(
            "domains",
            &fingerprint,
            state
                .domains
                .iter()
                .map(|(arn, domain)| (arn.clone(), domain_summary(domain))),
            input.next_token.as_deref(),
            input.max_results,
            MAX_LIST_DOMAINS_RESULTS,
        )
        .map_err(|error| {
            ListDomainsErrorModeled::ValidationException(ValidationException {
                message: Some(crate::state::page_refusal("ListDomains", error)),
            })
        })?;
```
The rule the suite states, and the rows it covers — tests/all/control_catalog.rs:245-246 and 295-306:
```
/// A `NextToken` resumes only the collection and filter that issued it; anything else
/// is refused as invalid input rather than answered from the wrong sequence.
```
```
        (
            "a token that is not an issued token",
            "/domains",
            "{}".to_string(),
            "deadbeef",
        ),
        (
            "a token that is not even hex",
            "/domains",
            "{}".to_string(),
            "not-a-token",
        ),
```
No row sends a case variant of an issued token, so nothing in the crate pins the current acceptance: the two refused rows above are unaffected by making the decode reject spellings the encoder never writes.

The decoder in use — Cargo.lock:3176-3180:
```
[[package]]
name = "hex"
version = "0.4.3"
```
`hex::encode` emits only `0-9a-f`; `hex::decode` treats `A-F` as the same digits as `a-f` (the same convention the framework's own hex readers use, e.g. vendor/smithy-server-runtime/src/lib.rs:107 `b'A'..=b'F' => Some(b - b'A' + 10)`), so uppercasing an issued token leaves the decoded bytes — collection, fingerprint and last key — identical.

`PaginationToken` carries no `@pattern` to stop it at the boundary (`smithy_query(query="operation", operation="ListDomains")`: `"NextToken": {"target": "PaginationToken", "httpQuery": "nextToken", "length": {"max": 1024}}`), so the mixed-case spelling reaches the handler.

## Steps to Reproduce

1. Start the emulator with this service mounted (as tests/all/control_catalog.rs's `Emulator` does).
2. POST `/domains?maxResults=1` with body `{}` and read `NextToken` from the 200 response — it is lowercase hex, because `encode_page_token` builds it with `hex::encode` (src/state.rs:645-650).
3. Uppercase that token (or flip any single `a`-`f` digit to upper case). `encode_page_token` can never emit this spelling, so it is not a token the service ever issued.
4. POST `/domains?maxResults=1&nextToken=<THE UPPERCASED TOKEN>` with body `{}`: the response is HTTP 200 and carries the second domain, because `hex::decode` (src/state.rs:659) accepts upper-case digits and yields the same `domains\u{1f}<fingerprint>\u{1f}<last key>` plaintext.
5. Compare with the token the crate does refuse — POST `/domains?nextToken=deadbeef` answers 400 `ValidationException` "nextToken is not a pagination token issued by a previous ListDomains call with the same parameters" (src/state.rs:687-690; tests/all/control_catalog.rs:295-300) — although neither token was issued by a previous call.
6. Repeat on `/objectives`, `/common-controls`, `/list-controls` and `/list-control-mappings`: all five go through the same `crate::state::page`/`decode_page_token` pair, so all five honour a spelling they never mint.
