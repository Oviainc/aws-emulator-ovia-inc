Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 12966d24554ed06eeaba7911ae25ff590cd06862

# The ListEnvironments cursor is parsed with an ambiguous separator the crate never enforces on the scope it encodes

_Filed: 2026-10-29T05:54:57Z_
_Operation: ListEnvironments | Class: pagination_

> _Note: this issue was identified against the code base at commit `12966d24554ed06eeaba7911ae25ff590cd06862`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEnvironments` mints its `NextToken` by concatenating the listing's scope (the Region) and the position (the environment name) with a single `:` separator, and decodes it by splitting at the FIRST `:` after the prefix. The crate's own comment on the separator asserts the split is unambiguous because the scope is a Region code matching `[a-z0-9\-]+`, which excludes `:`. Nothing in the crate establishes that assertion: `crate::state::request_region` — the ONE place the scope value comes from — takes the Region verbatim out of the request's SigV4 credential scope and never checks it against that shape.

So when a request's credential scope names a Region containing a `:`, `encode_cursor` writes `<prefix><region>:<name>` and `decode_cursor` splits the region in half: `issued_for` is only the part before the first `:`, the `issued_for != region` test fails, and the handler answers HTTP 400 `ValidationException` with `INVALID_NEXT_TOKEN_MESSAGE` ("NextToken is not a pagination token issued by this listing") — for a token that *this very listing* just issued. A caller walking pages is turned away mid-walk with a hard refusal that says the token is not one this listing minted, which is the opposite of the truth.

The defect is the parse, not the Region: the writer and the reader of the cursor disagree about whether the scope segment can carry the separator, and the crate's only source of that segment enforces nothing.

## Evidence

Side 1 — the crate asserts the scope cannot carry the separator (services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:1621-1625):
```
/// Separates the scope a cursor was issued for from the position it names. Neither
/// segment can carry it, so the split is unambiguous: a Region code (the shape
/// `EnvironmentArn`'s own Region segment, `[a-z0-9\-]+`) and an `EnvironmentName`
/// (`^[a-zA-Z][0-9a-zA-Z-_]*$`) both exclude it.
const CURSOR_SCOPE_SEPARATOR: char = ':';
```

Side 2 — the one place the scope comes from enforces nothing (state.rs:1357-1362):
```
/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
There is no `[a-z0-9\-]+` test anywhere on that value; `grep` for the shape in the crate finds it only inside doc comments.

Side 3 — the ambiguous round trip (state.rs:1644-1648 and state.rs:1668-1677):
```
pub fn encode_cursor(region: &str, name: &str) -> String {
    ::hex::encode(format!(
        "{CURSOR_PREFIX}{region}{CURSOR_SCOPE_SEPARATOR}{name}"
    ))
}
```
```
    let position = text.strip_prefix(CURSOR_PREFIX).ok_or(MalformedCursor)?;
    let (issued_for, name) = position
        .split_once(CURSOR_SCOPE_SEPARATOR)
        .ok_or(MalformedCursor)?;
    if issued_for != region {
        return Err(MalformedCursor);
    }
```
With `region = "us:east"`, `name = "myenv"`: the plaintext is `mwaa-after-environment:us:east:myenv`; after `strip_prefix` the remainder is `us:east:myenv`; `split_once(':')` yields `issued_for = "us"`, which `!= "us:east"` -> `MalformedCursor`.

Side 4 — the handler turns that into a hard 400 (services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/list_environments.rs:142-153):
```
        let region = crate::state::request_region(ctx);
...
        let after =
            crate::state::decode_cursor(&region, input.next_token.as_deref()).map_err(|_| {
                ListEnvironmentsErrorModeled::ValidationException(ValidationException {
                    message: Some(crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string()),
                })
            })?;
```

Test coverage: `tests/all/list_environments_cursor_scope.rs` pins the cursor-scope rule, but every row of its fixture uses a plain Region code (`harness.page(region, "?MaxResults=1")`, line 259), so no test exercises a scope value containing the separator; `tests/all/environment_lifecycle.rs:1102-1103` pages with `?MaxResults=...&NextToken=...` under the single `us-east-1` scope only.

## Steps to Reproduce

1. Create 2 or more MWAA environments through `CreateEnvironment`, signing every request with an `Authorization` header whose SigV4 credential scope names a region containing a colon (e.g. `Credential=AKIDTEST/20260101/us:east/airflow/aws4_request`) — `crate::state::request_region` accepts it verbatim.
2. Call `ListEnvironments?MaxResults=1` with that same header; the response carries a `NextToken` minted by `crate::state::encode_cursor("us:east", <first name>)`.
3. Call `ListEnvironments?MaxResults=1&NextToken=<that token>` with the same header.
4. Observe HTTP 400 `ValidationException` with the message "NextToken is not a pagination token issued by this listing", for a token this same listing issued: `decode_cursor` split the scope at the first `:`, compared `"us"` against `"us:east"` and returned `MalformedCursor`.
5. Compare with the same walk under a colon-free scope (`us-east-1`), where the second page is served normally — the only difference is whether the scope value carries the separator the comment at state.rs:1621-1625 says it cannot.
