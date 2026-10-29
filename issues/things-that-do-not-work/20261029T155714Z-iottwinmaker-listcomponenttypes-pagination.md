Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 3b2aca3b7a37a5147381645d3a0bbbf0f4b15f02

# decode_page_token honours nextToken hex spellings encode_page_token never mints (uppercase/mixed-case hex), so a token this listing did not mint is resumed instead of refused

_Filed: 2026-10-29T15:57:14Z_
_Operation: ListComponentTypes | Class: pagination_

> _Note: this issue was identified against the code base at commit `3b2aca3b7a37a5147381645d3a0bbbf0f4b15f02`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this service mints its `nextToken` with `crate::state::encode_page_token`, which renders the body with `::hex::encode` — lowercase hex, always. The matching reader, `crate::state::decode_page_token`, parses it with `::hex::decode`, which accepts `A`-`F` as readily as `a`-`f`. So a token whose body hex is upper-cased or mixed-case decodes to the same bytes and is served as a cursor, although `encode_page_token` can never have produced that spelling — exactly the "by hand" token the helper's own doc says the shape exists to make recognisable, and exactly the case `invalid_next_token_message` is the single refusal for.

The defect is in the shared helper, so it reaches every paginated operation of this crate (`ListComponentTypes`, `ListComponents`, `ListEntities`, `ListMetadataTransferJobs`, `ListProperties`, `ListScenes`, `ListSyncJobs`, `ListSyncResources`, `ListTagsForResource`, `ListWorkspaces`, `GetPropertyValue`, `GetPropertyValueHistory`, `ExecuteQuery`); it is filed against `ListComponentTypes` as the first listing whose pagination has no open issue. It is distinct from the already-filed ExecuteQuery pagination gap, which is about the ORDINAL inside a decoded token (`"+N"`, leading zeros, out-of-range), not about the hex envelope itself.

## Evidence

THE MINTER — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:4454-4474:
```
/// The opaque `nextToken` every paginated operation of this service mints.
///
/// One shape, shared by all of them so a token means the same thing wherever it
/// is read: hex (so the token is opaque to a caller and safe in a JSON body)
/// over an `envelope` naming the LISTING it belongs to, the resume `key` it
/// resumes at, and the `scope` the listing was taken in — the parent resource
/// whose page it is, which makes a token minted for another resource, by another
/// listing, or by hand recognisably not this listing's.
...
pub fn encode_page_token(envelope: &str, scope: &[&str], key: &str) -> String {
    let mut body = format!("{envelope}{key}");
    for part in scope {
        body.push('\n');
        body.push_str(part);
    }
    ::hex::encode(body)
}
```
`::hex::encode` emits only lowercase hex digits, so no token this service ever issues carries an uppercase one.

THE READER THAT WIDENS IT — src/state.rs:4476-4495:
```
/// The resume key inside a `nextToken` [`encode_page_token`] minted for this
/// `envelope` and `scope`, or `None` when the token is not one of them.
///
/// The key is split off first and each scope part is compared in turn, so a
/// token whose envelope, arity or scope disagrees is rejected rather than
/// resumed; ... A token carrying an EMPTY key is rejected too: a
/// listing never mints one, since a page is only resumed at a row that exists.
pub fn decode_page_token(envelope: &str, scope: &[&str], token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
```
`::hex::decode` accepts `0-9`, `a-f` AND `A-F`, so the spelling check the lowercase-only minter implies is never made — while the sibling rule one sentence away rejects an empty key for precisely the reason "a listing never mints one".

THE REFUSAL THAT SHOULD APPLY — src/state.rs:4616-4627:
```
/// The message for a `nextToken` [`decode_page_token`] does not recognise: the
/// ONE refusal every paginated operation of this service reports for a token it
/// did not mint, so the same failure reads the same way whichever page was
/// asked for.
...
pub fn invalid_next_token_message() -> String {
    "nextToken is not a token this listing minted".to_string()
}
```

THE CALLER — src/ops/list_component_types.rs:252-264 and :403-411:
```
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => Some(
                decode_next_token(token, &input.workspace_id)
                    .ok_or_else(|| invalid(crate::state::invalid_next_token_message()))?,
            ),
        };
```
```
fn encode_next_token(workspace_id: &str, component_type_id: &str) -> String {
    crate::state::encode_page_token(NEXT_TOKEN_PREFIX, &[workspace_id], component_type_id)
}

fn decode_next_token(token: &str, workspace_id: &str) -> Option<String> {
    crate::state::decode_page_token(NEXT_TOKEN_PREFIX, &[workspace_id], token)
}
```

TEST COVERAGE: tests/all/next_token_refusal.rs:15-23 enumerates the forged-token families the refusal is asserted over — "text that is not hex at all, hex of the wrong length, hex that decodes to other bytes, hex that decodes to no UTF-8, punctuation and markup, non-ASCII text, a token at the member's published maximum length, and a REAL token another listing minted" — and no test in tests/all anywhere upper-cases a token (grep for `to_uppercase` finds only tests/all/reserved_tag_keys.rs and a comment in tests/all/requested_map_refusal_order.rs), so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws"), then CreateComponentType twice (componentTypeId="com.example.a" and "com.example.b", each with a propertyDefinition carrying a dataType) so the listing has a page to leave over.
2. ListComponentTypes(workspaceId="ws", maxResults=1) -> 200 with one summary and a nextToken (lowercase hex of "iottwinmaker:componentTypes:1:com.example.b\nws").
3. Upper-case every hex digit of that nextToken (e.g. `token.to_uppercase()`) and call ListComponentTypes(workspaceId="ws", maxResults=1, nextToken=<the upper-cased token>).
4. Expected: 400 ValidationException "nextToken is not a token this listing minted" — encode_page_token mints only lowercase hex, so this spelling is not a token this listing issued, and the helper's doc says a token built "by hand" must be recognisably not this listing's. Actual: 200, the second page is served.
5. Repeat against any other paginated operation (e.g. ListScenes, ListTagsForResource, ListWorkspaces): the same shared helper accepts the same re-spelled token.
