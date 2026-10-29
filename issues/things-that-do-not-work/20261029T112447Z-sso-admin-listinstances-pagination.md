Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6eca256d767263dcce8cda9bce6eb5315c5aad3d

# decode_page_token honours uppercase/mixed-case hex spellings encode_page_token never writes, so a NextToken this service never issued is served as a page

_Filed: 2026-10-29T11:24:47Z_
_Operation: ListInstances | Class: pagination_

> _Note: this issue was identified against the code base at commit `6eca256d767263dcce8cda9bce6eb5315c5aad3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate resolves its `NextToken` through ONE shared gate, `crate::state::cursor_after` -> `decode_page_token` (`ListInstances`, `ListApplicationProviders`, `ListPermissionSets`, `ListApplications`, `ListRegions`, `ListTrustedTokenIssuers`, `ListAccountAssignments`, the two `…ForPrincipal` listings, the status listings, `ListTagsForResource`, …). That gate documents its own rule: the cursor is SELF-IDENTIFYING, so a token this service never issued — including one that happens to be valid hex — is REJECTED rather than resumed from.

The ONE minting site, `encode_page_token`, writes `::hex::encode(...)`, which emits lowercase hex digits only (`0`-`9`, `a`-`f`). The decoder hands the token to `::hex::decode`, which accepts `A`-`F` as well as `a`-`f`, so every upper- or mixed-case respelling of an issued token decodes to the same `ssoadmin:v1:<scope>:<key>` string and is served as a page. The modelled `Token` pattern `^[-a-zA-Z0-9+=/_]*$` admits the uppercase letters, so the boundary validator passes such a value straight through to the handler.

The result is the opposite of what the helper, the handlers' comments and the crate's own test state: a value this service never handed out is accepted (an error turned into a success, a page positioned from a client-fabricated token) instead of earning the one `ValidationException` the crate answers for every other token it did not issue. On `ListInstances` the fabricated token answers a 200 empty page where a 400 is due; on an enumeration with more than one row (e.g. `ListApplicationProviders`, whose catalog the crate's own test asserts holds more than one entry) it serves the real next page.

## Evidence

The rule the gate states — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:4604-4611 (doc on `encode_page_token`):
```
/// The cursor is SELF-IDENTIFYING — it carries a version marker and that scope, so a
/// token this service never issued (including one that happens to be valid hex) and a
/// token issued for a DIFFERENT scope (another operation, another instance, another
/// account or permission-set filter) are both rejected rather than silently resuming
/// inside an enumeration whose keys mean something else.
```

The ONE minting site, state.rs:4612-4614 — `hex::encode` writes lowercase digits only:
```
pub fn encode_page_token(scope: &str, key: &str) -> String {
    ::hex::encode(format!("ssoadmin:v1:{scope}:{key}"))
}
```

The decoder that contradicts it, state.rs:4617-4622 — `hex::decode` accepts `A`-`F` as well as `a`-`f`, so an uppercased token decodes to the very same bytes:
```
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("ssoadmin:v1:{scope}:"))
        .map(str::to_string)
}
```
(`hex` is this crate's own dependency, Cargo.toml:26, declared "Hex-encodes pagination cursors, so a `NextToken` stays inside the model's `^[-a-zA-Z0-9+=/_]*$` pattern".)

The single gate every handler uses, state.rs:4629-4639 — `Err` is the refusal, "answering an empty page for a token this service did not issue would silently end a caller's enumeration early"; called from src/ops/list_instances.rs:171-180 and from the `cursor_after(&scope, input.next_token.as_deref())` line of every other paginated handler.

The modelled member that lets the uppercase spelling past the boundary — `smithy_query(query="operation",operation="ListInstances")`: `"NextToken": {"target": "Token", "length": {"min": 0, "max": 2048}, "pattern": "^[-a-zA-Z0-9+=/_]*$"}`.

Test coverage: tests/all/list_instances_pagination.rs:139-167 states the rule — "A `NextToken` this service did not issue FOR THIS ENUMERATION is a bad request, not an empty page" — and enumerates only three rows: `"not-a-token"`, `"73736f61646d696e3a76313a7065726d697373696f6e536574733a78"` ("hex that decodes to another scope") and `""`. No test names the upper- or mixed-case respelling of an ISSUED token, so nothing pins the current acceptance.

## Steps to Reproduce

1. Start the emulator and POST / with `X-Amz-Target: SWBExternalService.ListInstances` and body `{}`; note the issued `NextToken` (lowercase hex, e.g. `73736f...`).
2. Repeat the call with that token upper-cased (`.to_ascii_uppercase()`, e.g. `73736F...`): the emulator answers 200 with a page, although `encode_page_token` can never have written that spelling — every other token it did not issue is refused 400 `ValidationException` with `NextToken is not a pagination token issued by this service for this enumeration`.
3. POST / with `X-Amz-Target: SWBExternalService.ListApplicationProviders` and `{"MaxResults":1}`; take the returned `NextToken`, upper-case its hex digits and send it back: the second provider is served as a page, so a client-fabricated token positions a real enumeration.
4. Mix the case of a single digit of any issued token (e.g. one `a` -> `A`) and resend: still 200, since `hex::decode` is case-insensitive while `hex::encode` is not.
