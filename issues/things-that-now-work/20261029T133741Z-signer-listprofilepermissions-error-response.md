Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 07e288609021e6571eb22a2173d293968d6da2ab

# ListProfilePermissions echoes the caller's nextToken whole in its refusal, the no-echo/ceiling rule every sibling token refusal in this crate applies

_Filed: 2026-10-29T13:37:41Z_
_Operation: ListProfilePermissions | Class: error-response_

> _Note: this issue was identified against the code base at commit `07e288609021e6571eb22a2173d293968d6da2ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paginated reads of this crate refuse a `nextToken` the service did not hand out with the same modelled 400 and the same `code` (`crate::state::ERROR_CODE_INVALID_NEXT_TOKEN`). Three of them deliberately state the refusal WITHOUT quoting the caller's value, and `list_signing_profiles.rs` writes the rule out: "The refused value is not echoed". `ListProfilePermissions` alone interpolates the caller's bytes into the message verbatim: `format!("Invalid next token provided: {token}")`.

Nothing bounds that value. The model binds the member as a bare string (`"nextToken": {"target": "String", "httpQuery": "nextToken"}` — no `@length`, no `@pattern`), so the boundary validator imposes no size and the handler reaches its refusal for a token of any length (a token no statement id matches is refused at `list_profile_permissions.rs:214-218`, and for a profile that holds no policy at all at line 197-199). A `GET /signing-profiles/<name>/permissions?nextToken=<1 MiB>` is therefore answered with a 400 whose `message` carries that whole megabyte back — exactly the outcome this crate states its echo ceiling exists to prevent (`MAX_STATED_VALUE` / `stated_value`, used by `platform_id_pattern_refusal`).

So one paginated read of this service quotes an unbounded request value in an error body while its three siblings, refusing the same condition with the same code, quote nothing.

## Evidence

The outlier — services/service-com-amazonaws-signer-wallabyservice/src/ops/list_profile_permissions.rs:124-134:
```
/// The modelled 400 for a `nextToken` that is not one this service handed out.
///
/// `code` names the condition a paginating client resets on, as the service's
/// other paginated list words it.
fn invalid_next_token(token: &str) -> ListProfilePermissionsError {
    ListProfilePermissionsErrorModeled::ValidationException(ValidationException {
        message: Some(format!("Invalid next token provided: {token}")),
        code: Some(crate::state::ERROR_CODE_INVALID_NEXT_TOKEN.to_string()),
    })
```
The rule the siblings state and follow — src/ops/list_signing_profiles.rs:132-144:
```
/// The modelled 400 for a `nextToken` that is not one this listing handed out.
///
/// `code` names the condition a paginating client resets on, as the service's
/// other paginated read words it. The refused value is not echoed: a client that
/// held a token from another listing learns that the token is not valid here, and
/// nothing of what the token decodes to is reported back.
fn invalid_next_token() -> ListSigningProfilesError {
    ListSigningProfilesErrorModeled::ValidationException(ValidationException {
        message: Some("Invalid next token provided".to_string()),
        code: Some(crate::state::ERROR_CODE_INVALID_NEXT_TOKEN.to_string()),
    })
```
The same no-echo wording in the other two: src/ops/list_signing_jobs.rs:136-138 (`message: Some("Invalid next token provided".to_string())`) and src/ops/list_signing_platforms.rs:125-127 (same).

The ceiling this crate states for any echoed request value — src/state.rs:1503-1513:
```
/// The longest rejected value a clause states in full.
///
/// The value between the quotes is the caller's OWN bytes ...
/// A value with no ceiling at all would let one oversized
/// query parameter become an equally large error body, so the ceiling is set
/// where the shared refusal-wording module sets the largest echo it restores
/// into a clause (16 KiB) ...
const MAX_STATED_VALUE: usize = 16 * 1024;
```
and where it is applied (src/state.rs:1556-1562, `platform_id_pattern_refusal`): `value: Some(stated_value(value)),`.

The member carries no size bound to lean on: `smithy_query(query="operation", operation="ListProfilePermissions")` binds `"nextToken": {"target": "String", "httpQuery": "nextToken"}`.

Test coverage: no test in tests/ asserts this message at all (grep for "Invalid next token" in tests/ returns no matches), so neither dropping the echo nor routing it through `stated_value` changes any existing expectation.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. PutSigningProfile PUT /signing-profiles/Perm_Profile with platformId AWSLambda-SHA384-ECDSA, then AddProfilePermission POST /signing-profiles/Perm_Profile/permissions with action signer:GetSigningProfile, a principal and a statementId, so the profile holds a policy.
3. Issue GET /signing-profiles/Perm_Profile/permissions?nextToken=<1048576 'a' characters> (a value no statement id matches).
4. Observe HTTP 400 ValidationException whose message is `Invalid next token provided: aaaa…(1 MiB)…` — the error body is as large as the request parameter.
5. Issue GET /signing-profiles?nextToken=<the same 1 MiB value> and observe the sibling answer: `Invalid next token provided`, with the same code and no echo at all.
6. Compare the two: the same refusal of the same member class is unbounded on one paginated read and quotes nothing on the other three.
