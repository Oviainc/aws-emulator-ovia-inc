Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3b8e3153ba12b36fed5bedf556b95360903c1151

# decode_next_token honours NextToken spellings encode_next_token never mints (uppercase hex, a signed issue instant), so a token this service never issued is served as a page

_Filed: 2026-10-29T11:34:48Z_
_Operation: ListSchemas | Class: pagination_

> _Note: this issue was identified against the code base at commit `3b8e3153ba12b36fed5bedf556b95360903c1151`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this service resolves its `NextToken` through the one shared reader `crate::state::decode_next_token`, whose own documentation says it answers `None` for "one this service never minted". The reader is nonetheless more permissive than the minter on two of the token's three fields, so values the minter can never produce are accepted and the listing is resumed instead of refused:

1. `encode_next_token` renders the selection and the resume key with `::hex::encode`, which emits LOWER-CASE hex only; `decode_next_token` reads them back with `::hex::decode`, which accepts upper-case and mixed-case digits equally. A token whose two hex halves are upper-cased is therefore honoured, although no listing of this service ever handed one out.

2. `encode_next_token` renders the issue instant with `{issued_at_seconds:016x}` — exactly sixteen lower-case hex digits; `decode_next_token` reads it with `u64::from_str_radix(encoded_issued_at, 16)`, which accepts a leading `+`, upper-case digits, fewer than sixteen digits and extra leading zeros. So `+000000006553f100`, `6553F100` and `000000000000000000006553f100` are all read as the same instant and all resume the page.

This is the same "the exact rendering is what names it" rule the crate already applies, deliberately and with a stated reason, to the OTHER opaque label a request of this service carries: `Schema::version_number` screens the spelling of `SchemaVersion` before parsing it precisely because `u64`'s parser "accepts a leading `+` and so would let a signed label name a version", and it rejects zero padding for the same reason. The token reader skips that screening, so the crate answers the same question — may a spelling the service never renders name the thing it merely parses as? — two contradictory ways.

The handler then reports success where its own refusal text says it will not: `crate::state::invalid_next_token()` states "The NextToken provided is not a token this service issued for this listing."

ListSchemas is the operation cited because it is the first listing on the checklist that calls `crate::state::page_by_key` (list_schemas.rs:227), but the reader is shared with ListRegistries, ListSchemaVersions, ListDiscoverers and SearchSchemas, so the fix belongs in `decode_next_token`.

## Evidence

The rule the reader states for itself — services/service-com-amazonaws-schemas-schemas/src/state.rs:3358-3362:
```
/// Read back the key a `NextToken` [`encode_next_token`] minted for `selection`
/// resumes after, as of `now`. `None` for anything else: a token minted for a
/// different selection, one this service never minted, and one minted more than
/// [`NEXT_TOKEN_TTL`] before `now`.
fn decode_next_token(selection: &str, token: &str, now: SystemTime) -> Option<String> {
```

What the minter can actually render — state.rs:3346-3356:
```
pub fn encode_next_token(selection: &str, last_key: &str, issued_at: SystemTime) -> String {
    let issued_at_seconds = issued_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "{}.{}.{issued_at_seconds:016x}",
        ::hex::encode(selection),
        ::hex::encode(last_key),
    )
}
```
(`::hex::encode` emits lower-case digits; `{:016x}` emits exactly sixteen lower-case digits and no sign.)

What the reader accepts — state.rs:3369-3381:
```
    if ::hex::decode(encoded_selection).ok()? != selection.as_bytes() {
        return None;
    }
    let issued_at = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(
            u64::from_str_radix(encoded_issued_at, 16).ok()?,
        ))?;
    ...
    String::from_utf8(::hex::decode(encoded_key).ok()?).ok()
```
`::hex::decode` accepts `A`-`F` as well as `a`-`f`, and `u64::from_str_radix(.., 16)` accepts a leading `+`, upper-case digits and any number of leading zeros — so none of the three fields is held to the spelling above.

The contrasting rule the SAME crate applies to its other opaque label — state.rs:1412-1433:
```
/// The ONE definition of how a request's version label is read, so every reader
/// and the delete resolve it the same way: a label names a version only when it is
/// exactly how this service RENDERS that version — the decimal digits of the
/// number and nothing else — so any other spelling (surrounding space, a sign,
/// zero padding, a decimal point, a word, a number too large to be a version)
/// names no version at all.
fn version_number(requested: &str) -> Option<u64> {
    // Screen the spelling before handing the digits to `u64: FromStr`, which on its
    // own accepts a leading `+` and so would let a signed label name a version.
    if requested.is_empty() || !requested.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Zero padding is the only way an all-digit label can still differ from the
    // rendering, which is why the rendering itself need not be built to compare.
    if requested.len() > 1 && requested.starts_with('0') {
        return None;
    }
```

The refusal the listing promises for a token it did not issue — state.rs:4076-4084:
```
pub fn invalid_next_token() -> BadRequestException {
    let hours = NEXT_TOKEN_TTL.as_secs() / 3600;
    bad_request(format!(
        "The NextToken provided is not a token this service issued for this listing. ..."
```
and the call site that answers it — services/service-com-amazonaws-schemas-schemas/src/ops/list_schemas.rs:227-238:
```
        let page = crate::state::page_by_key(
            matching.into_iter(),
            &selection,
            input.next_token.as_deref(),
            page_size,
            now,
        )
        .map_err(|_| {
            ListSchemasError::from(ListSchemasErrorModeled::BadRequestException(
                crate::state::invalid_next_token(),
            ))
        })?;
```

Test coverage: `tests/all/next_token_lifetime.rs` is the crate's token suite and it builds every synthetic token through the minter itself (`encode_next_token(&listing.selection, &listing.resume_after, SystemTime::now() - age)`, next_token_lifetime.rs:230-234), so no case ever presents an alternately-spelled token; `tests/all/error_body_codes.rs:336-344` only checks that the literal `not-a-token` is refused. Nothing in the crate pins or justifies the leniency.

## Steps to Reproduce

1. Create a registry and two schemas in it, so a listing of the registry's schemas has more than one page.
2. Call `GET /v1/registries/name/<registry>/schemas?limit=1` and keep the returned `NextToken` — it is three `.`-separated fields of lower-case hex, the last exactly sixteen digits (state.rs:3351-3355).
3. Upper-case every hex digit of the token and present it: `GET /v1/registries/name/<registry>/schemas?limit=1&nextToken=<UPPER-CASED TOKEN>`. It is served with the next page (HTTP 200), although `::hex::encode` never emitted those digits, so no listing of this service ever issued that token.
4. Take the original token and replace its third field with the same value written with a leading `+` (`+000000006553f100` style), or with the leading zeros stripped, or with upper-case hex digits. Present it the same way: each is again served with the next page.
5. Compare with `decode_next_token`'s own promise (state.rs:3358-3361: `None` for "one this service never minted") and with the refusal `invalid_next_token()` words (state.rs:4079) — a 400 is documented, a 200 is served.
6. Contrast with the same crate's treatment of the other opaque label: `DELETE /v1/registries/name/<registry>/schemas/name/<schema>/version/+1` is refused 404 because `Schema::version_number` (state.rs:1420-1433) screens the spelling before parsing it.
