Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 2d54232ecf89d09f7d71c64df7cdb47dba635b50

# The portal's pre-handler gate authenticates every request when the store is unreachable, then answers the absent bearer credential as a missing parameter instead of leaving the framework's answer or the modelled 401

_Filed: 2026-11-02T04:03:13Z_
_Operation: Logout | Class: error-response_

> _Note: this issue was identified against the code base at commit `2d54232ecf89d09f7d71c64df7cdb47dba635b50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EmulatedService::presents_a_usable_session` is the portal's pre-handler authentication gate. When `crate::state::service_state` answers `None` (the service built on something other than the shared `InMemoryEngine`, which the crate is generic over), the gate returns `true` — i.e. it reports that the request DID present a usable sign-in session — and its own comment justifies that with "the gate claims nothing here and the framework's own answer stands".

Neither half holds. (1) The framework's own answer does NOT stand: `override_routing_error` has already committed to answering (`let absent = absent_required_members(req)?` matched one of this service's four routes), so after the gate says `true` the seam falls through to its service-authored 400 `InvalidRequestException` naming the absent `@required` member. On `POST /logout` the only `@required` http-bound member IS the bearer credential, so a request presenting NO credential at all is answered 400 "accessToken is missing" — the exact case `crate::state::unauthorized_response` exists for and documents itself as covering. (2) Returning `true` makes the gate assert the opposite of what it cannot determine: the crate's other no-store branches (every handler) answer a modelled refusal rather than proceeding as if the check had passed, and they pick `TooManyRequestsException`, the one modelled error that does not blame the caller.

The branch is documented UNREACHABLE with the engine the router mounts, which is why this is LOW: no response a client of the deployed emulator can observe changes today. It is nevertheless reachable through the crate's own public, engine-generic constructor (`EmulatedService::<Eng>::new`), and the contradiction is between two files of this crate. Filed against `Logout` because its route makes the defect maximal — the credential is its only required member, so the gate's no-store branch converts the authentication refusal into a parameter complaint with nothing else in the request to blame.

## Evidence

The gate's no-store branch and the claim it makes — `services/service-com-amazonaws-sso-swbportalservice/src/service.rs:264-277`:
```rust
    fn presents_a_usable_session(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        let Some(token) = presented_access_token(req) else {
            return false;
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // UNREACHABLE with the engine the router mounts (see the note in
            // `ops/list_accounts.rs`): with no store to resolve the token against there
            // is nothing to authenticate it with, so the gate claims nothing here and
            // the framework's own answer stands.
            return true;
        };
```

Why the framework's answer does NOT stand — the same file's seam continues to a service-authored 400 once the gate returns `true` (`src/service.rs:361-396`):
```rust
        let absent = absent_required_members(req)?;
...
        if !self.presents_a_usable_session(req) {
            return crate::state::unauthorized_response(self.entry.protocol());
        }
...
        let violations: Vec<aws_smithy_schema_server::Violation> = absent
            .into_iter()
            .map(|member| aws_smithy_schema_server::Violation {
                path: member,
                kind: aws_smithy_schema_server::ViolationKind::MissingRequiredMember,
                value: None,
            })
            .collect();
...
        Some(self.entry.refusal_response(
            aws_smithy_schema_server::ProtocolError::Invalid(violations),
            Some(Refused {
                code: "InvalidRequestException".to_string(),
                message,
            }),
        ))
```

The rule the no-credential case is supposed to follow, stated by this crate's state module — `src/state.rs:74-80`:
```
/// A request that presents no bearer credential at all never reaches a handler: the REST
/// router selects a route only when every `@required` http-bound member is on the
/// request, so the absent credential ends the request before any handler's gate could
/// run. `crate::service` answers that case through here, so the authentication refusal a
/// caller sees is the same one whether the credential was absent or merely named no
/// usable session.
```
and the authentication-first rule immediately above the gate call — `src/service.rs:363-370`:
```
        // AUTHENTICATION FIRST. The portal has no other caller identity: ...
        // a request presenting no usable session is an unauthenticated request and is
        // refused as one — `UnauthorizedException`, with the status its model carries —
        // whatever else the request did or did not carry. That covers the request whose
        // absent member IS the credential: an absent credential is not a malformed
        // parameter, ...
```
The no-store branch makes that request a malformed parameter.

The sibling choice every handler makes in the SAME situation — `src/ops/logout.rs:170-178`:
```rust
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // UNREACHABLE with the engine the router mounts (see `ListAccounts` for the
            // full note): answered rather than panicked, and the 429 is the only
            // modelled error that does not blame the caller.
            return Err(LogoutErrorModeled::TooManyRequestsException(TooManyRequestsException {
                message: Some("the SSO portal store is unavailable".to_string()),
            })
            .into());
```
(identically `src/ops/list_accounts.rs:153-167`, `src/ops/list_account_roles.rs:152-162`, `src/ops/get_role_credentials.rs:152-162`.) A handler never proceeds as though the check passed; the gate does.

That `Logout`'s only required http-bound member is the credential — `OPERATIONS.md:49-52`:
```
## Logout

- **Route:** `POST /logout -> 200`
- **Input:** `accessToken` (string, @httpHeader(x-amz-sso_bearer_token), required)
```
and the hand-held table the seam matches it with — `src/service.rs:147`: `("POST", "/logout", &[ACCESS_TOKEN]),`.

Test coverage: the absent-credential rule IS pinned, but only on the `InMemoryEngine` (`tests/all/bearer_token_gate.rs:185-186` presents `Credential::Absent` and `tests/all/bearer_token_gate.rs:359-397` asserts "an absent credential answers exactly what an unknown token answers", both via `Portal::new()`, which builds `engine::InMemoryEngine` at `tests/all/portal_session.rs:86`). No suite constructs `EmulatedService` over any other engine type, so the `return true;` branch is unpinned by anything.

## Steps to Reproduce

1. Build the service over an engine that is not the shared `InMemoryEngine` — the crate is generic over it (`pub struct EmulatedService<Eng: Send + Sync + 'static>`, `src/service.rs:236-254`): e.g. `struct NoStore; let svc = EmulatedService::new(std::sync::Arc::new(NoStore));` and `router.register_service(svc)` on a `MultiServiceRouter`.
2. Dispatch `POST https://portal.sso.us-east-1.amazonaws.com/logout` with header `host: portal.sso.us-east-1.amazonaws.com` and NO `x-amz-sso_bearer_token` header (the request a client makes before it has signed in).
3. Observed: the REST router selects no route (the `@required` header is absent), `override_routing_error` runs, `absent_required_members` yields `["accessToken"]`, `presents_a_usable_session` returns `true` from its no-store branch (`src/service.rs:273`), and the service answers 400 `InvalidRequestException` reporting `accessToken` as a missing required member.
4. Expected per the branch's own comment (`src/service.rs:271-272`, "the gate claims nothing here and the framework's own answer stands") and per `src/state.rs:74-80` (`crate::service` answers the absent-credential case through `unauthorized_response`, so it sees the same refusal as a token naming no usable session): either the framework's own routing answer, or the modelled 401 `UnauthorizedException` "Session token not found or invalid" — never a 400 blaming the caller's parameters.
5. Contrast the handlers on the same engine: dispatch the same request WITH any bearer-token header, so a route is selected and `Logout::handle` runs — it answers the modelled 429 `TooManyRequestsException` "the SSO portal store is unavailable" (`src/ops/logout.rs:170-178`) instead of proceeding as if the store had answered.
