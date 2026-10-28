Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# decode_page_token accepts any hex-of-text cursor, so a forged or cross-operation nextToken silently answers an empty page instead of the ValidationException the crate promises

_Filed: 2026-10-28T20:55:35Z_
_Operation: ListPolicies | Class: pagination_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::decode_page_token` is documented as the gate that tells a cursor this service minted from one it did not, and all three implemented list handlers rely on that claim to answer a 400 `ValidationException` for a bad cursor. The implementation only checks that the token is valid hex whose bytes are valid UTF-8 — it never checks that the decoded value is a key this service could have minted, nor that it belongs to the resource kind (or the operation) being paged. The decoded string is then used solely as a `>=` lower bound on the keys enumerated, so any hex-of-text token is accepted and silently truncates (or empties) the listing.

Two concrete consequences:

1. A hand-crafted token, e.g. hex of `"zz"` (`"7a7a"`), decodes fine and sorts above every minted id (`SP…`, `PT…`, `IS…`), so `ListPolicies`/`ListIdentitySources` answer HTTP 200 with an empty item list and no `nextToken` — the caller concludes the store is empty. The handler comment says this exact outcome must not happen.
2. A cursor one operation issued is accepted by another: a `ListPolicies` cursor is the hex of a `SP…` policy id; submitted to `ListIdentitySources` for the same store it decodes to `SP…`, and because every identity-source id is `IS…` (`'I' < 'S'`), the `>=` filter drops every source. The response is a 200 with `identitySources: []` and no `nextToken`, for a store that holds identity sources.

The existing tests pin only the cursors that fail to decode at all (non-hex, odd-length hex, hex that is not UTF-8); the hex-of-text case that decodes to a never-minted key is the one that slips through, and the test's own stated rule is that it must be refused.

## Evidence

The documented contract, `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:1258`:

```
/// Decode a cursor produced by [`encode_page_token`]. `None` when the token was
/// not produced by this service, which the caller refuses as invalid.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}
```

Nothing here ties the decoded value to anything this service minted — `hex::decode("7a7a")` succeeds and `"zz"` is valid UTF-8, so `Some("zz")` is returned for a cursor `encode_page_token` never produced.

The handler that relies on that claim, `src/ops/list_policies.rs:149`:

```
        // A cursor this service did not mint is a bad request, not an empty page:
        // answering an empty page would hide the client's mistake.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListPoliciesErrorModeled::ValidationException(crate::ops::validation_exception([
                    crate::ops::InvalidInput::new(
                        crate::ops::paths::NEXT_TOKEN,
                        "it is not a cursor this service issued",
                    ),
                ]))
            })?),
        };
```

and the only use made of the decoded value, `src/ops/list_policies.rs:176`:

```
            .filter(|(policy_id, _)| {
                resume_from
                    .as_ref()
                    .is_none_or(|from| policy_id.as_str() >= from.as_str())
            })
```

The same two spans are duplicated verbatim in `src/ops/list_identity_sources.rs:152`/`:179` and `src/ops/list_policy_store_aliases.rs:157`/`:195`.

The crate's own test states the rule and covers only the decode-failure rows, `tests/all/list_identity_sources.rs:491`:

```
/// A `policyStoreId` that names no live store is the model's 404, and a
/// `nextToken` this service did not mint is a 400 naming the member — not an
/// empty page, which would hide the client's mistake.
```

`tests/all/list_identity_sources.rs:538`:

```
    // Rows differ along HOW the cursor is wrong: not this service's encoding at
    // all, an odd number of hex digits, and hex that decodes to bytes which are
    // not text.
    for (case, token) in [
        ("not a cursor", "notacursor"),
        ("odd-length hex", "953"),
        ("hex that is not text", "ff"),
    ] {
```

No row submits hex that decodes to printable text, which is the case the gate lets through.

## Steps to Reproduce

1. CreatePolicyStore, then CreatePolicy three times in it so ListPolicies has items to report.
2. Call ListPolicies with {"policyStoreId": "<id>", "nextToken": "7a7a"} (the hex of "zz", a cursor this service never minted). Observe HTTP 200 with "policies": [] and no nextToken, instead of the 400 ValidationException naming /nextToken that the handler comment and tests/all/list_identity_sources.rs:491 state is required.
3. CreateIdentitySource in the same store, then call ListPolicies with maxResults 1 and copy the returned nextToken (hex of a SP… policy id).
4. Submit that token to ListIdentitySources for the same store: {"policyStoreId": "<id>", "nextToken": "<ListPolicies cursor>"}. Observe HTTP 200 with "identitySources": [] and no nextToken, although the store holds an identity source.
