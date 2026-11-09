Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f019f7521c37cda1d7c01ced1e447769bd151b6c

# override_refusal words a bound-violating max_result as 400 without applying the portal's authentication-first rule, so an unusable token answers 400 where the same request with a bad next_token answers 401

_Filed: 2026-10-30T22:32:00Z_
_Operation: ListAccounts | Class: error-response_

> _Note: this issue was identified against the code base at commit `f019f7521c37cda1d7c01ced1e447769bd151b6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The portal has exactly one caller identity — the sign-in session the bearer token names — and this crate states the rule for itself twice: authentication is answered FIRST, "whatever else the request did or did not carry". `EmulatedService::override_routing_error` implements that rule (it calls `self.presents_a_usable_session(req)` before wording anything about a missing parameter), but the sibling seam in the SAME file, `EmulatedService::override_refusal`, does not: it rewords every `Refusal::Invalid` as 400 `InvalidRequestException` without ever asking whether the request presents a usable session, even though `presents_a_usable_session` is a method on the very same type and `crate::state::unauthorized_session()` / `unauthorized_response()` exist precisely to word the one refusal.

That seam is the one the framework uses for a value outside a declared bound (`maxResults` outside the modeled `MaxResultType` range 1..100): the router matches the operation, boundary validation fails, the handler — and therefore the handler's own 401 gate — never runs, and `override_refusal` answers 400. So `GET /assignment/accounts?max_result=101` presenting a token that names no session (unknown, logged out or expired) is answered 400 `InvalidRequestException`, telling an unauthenticated caller that the service read and judged its parameter.

The inconsistency is visible inside the service, without reference to AWS: the OTHER unreadable-parameter class of the same operation — a `next_token` this service never issued — is checked inside the handler, after its gate, so the identical unusable token answers 401 there (pinned). Two unreadable parameters of one request shape therefore get opposite precedences from the same operation. `ListAccountRoles` inherits the same seam (`max_result` is modeled on it too); this is filed once because `override_refusal` is service-wide and single-homed in `src/service.rs`.

Secondary symptom of the same seam and class: the comment on the `None` arm claims the remaining refusal class has no modeled error — "Every other refusal class (an unauthenticated caller) keeps the framework's default wording: this service's model declares no error for it" — but an unauthenticated caller is exactly what the model's `UnauthorizedException` (HTTP 401) states, and the crate already words it in `crate::state::unauthorized_session` / `unauthorized_response`, documented as "The portal's one authentication refusal" answered by "Every handler". So the one seam that could reword a framework-level authentication refusal declines to, on a premise the crate's own state module contradicts.

This is a different root cause from the already-filed ListAccounts `wrong-behavior` issue (slash-padded paths missing the hand-rolled `ROUTES` tables, which short-circuits `override_routing_error` before the gate). Here the path matches exactly, the operation IS named, and the gap is that the `override_refusal` seam never consults the session at all.

## Evidence

The seam that skips the gate — `services/service-com-amazonaws-sso-swbportalservice/src/service.rs:336-352`:
```rust
    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // This service states a request it will not read as `InvalidRequestException`,
        // ...
        if let Refusal::Invalid(violations) = refusal {
            return Some(Refused {
                code: "InvalidRequestException".to_string(),
                message: aws_smithy_schema_server::envelope(violations)
                    .unwrap_or_else(|| "the request is not valid".to_string()),
            });
        }
        // Every other refusal class (an unauthenticated caller) keeps the
        // framework's default wording: this service's model declares no error for it.
        None
    }
```
No call to `self.presents_a_usable_session(..)` anywhere in it.

The rule the sibling seam states and applies — `src/service.rs:363-372`:
```rust
        // AUTHENTICATION FIRST. The portal has no other caller identity: everything it
        // answers comes out of the sign-in session the request's bearer token names, so
        // a request presenting no usable session is an unauthenticated request and is
        // refused as one — `UnauthorizedException`, with the status its model carries —
        // whatever else the request did or did not carry. ...
        if !self.presents_a_usable_session(req) {
            return crate::state::unauthorized_response(self.entry.protocol());
        }
```

The same rule asserted by the crate's own test docs — `tests/all/portal_session.rs:799-802`:
```
/// RULE: a request from an AUTHENTICATED caller that this service cannot read is 400
/// `InvalidRequestException` — an absent required parameter, a `maxResults` outside the
/// modeled 1..100 range, or a `nextToken` it did not issue. (A request presenting no
/// usable session never gets this far: it is 401, whatever else it carries.)
```
and `tests/all/bearer_token_gate.rs:6-13`:
```
//! RULE: the portal authenticates every request by the sign-in session its bearer token
//! names, and does so BEFORE it reads anything else from the request; a request that
//! presents no usable session ... is refused as unauthenticated (HTTP 401
//! `UnauthorizedException`, with the gate's one message) on every bound route, whatever
//! else the request carries or omits.
```
The same file then admits the hole it leaves out of its sweep — `tests/all/bearer_token_gate.rs:22-26`:
```
//! A request carrying a VALUE outside a modelled constraint is not a row here: that
//! value is refused by the framework's own boundary validation, which runs over a
//! request the router already matched to an operation and so never reaches this
//! service's credential gate.
```

That `Refusal::Invalid` really is the out-of-range class, and it is rendered without running the handler — `vendor/smithy-server-runtime/src/pre_dispatch.rs:149-159`:
```rust
pub enum Refusal {
    /// One or more values failed the rules the model declares — a member the
    /// shape requires was absent, or a value fell outside a declared bound.
    Invalid(Vec<Violation>),
```
and `vendor/smithy-server-runtime/src/operation.rs:490-497`:
```rust
            // The framework-level failure tail: a refusal (an absent required
            // member, or a value outside a declared bound) is reworded by the
            // service through its own renderer ...
            let chosen = crate::refusal_of(&e).and_then(|r| svc.override_refusal(&r));
```

The contrasting path that DOES answer 401 for the same unusable token, because its check lives in the handler after the gate — `src/ops/list_accounts.rs:173-205` (gate at 173-180, cursor refusal at 198-205) — pinned at `tests/all/bearer_token_gate.rs:113-117` (`"ListAccounts with a next_token this service never issued"`, swept over every unusable credential and asserted 401 by `assert_unauthenticated`) and `tests/all/unauthorized_rejection.rs:65-68`.

Secondary symptom: the premise `src/service.rs:349-351` denies is the crate's own modeled 401 — `src/state.rs:58-70`:
```rust
pub const INVALID_SESSION_MESSAGE: &str = "Session token not found or invalid";

/// The portal's one authentication refusal, for a presented access token that names no
/// usable sign-in session (unknown, logged out or expired alike).
///
/// Every handler returns THIS value ...
pub fn unauthorized_session() -> crate::ops::types::UnauthorizedException {
```

Test coverage: the 400 rows for `max_result` are pinned ONLY for an authenticated caller — `tests/all/portal_session.rs:804-820` signs in `tok-live` (`portal.sign_in("tok-live", &every_account())`) before sweeping `("0"|"101"|"-1", Refused("InvalidRequestException"))`, and the test's own doc scopes the rule to "a request from an AUTHENTICATED caller". No test in the crate pairs an out-of-range `max_result` with an unusable token, so answering that combination 401 breaks nothing in the suite.

## Steps to Reproduce

1. Seed the portal exactly as `tests/all/portal_session.rs:85-114` does: `put_account` for the fixture accounts and `put_session("tok-live", …, [account ids], ttl)`, with `EmulatedService` registered on a `MultiServiceRouter`.
2. Dispatch `GET https://localhost:9297/assignment/accounts?max_result=101` with `host: localhost:9297` and `x-amz-sso_bearer_token: a-token-that-was-never-issued` (a token no session was ever minted for).
3. Observed: 400 with `__type: InvalidRequestException` — the router named the operation, boundary validation rejected the out-of-range value, `override_refusal` (`src/service.rs:336-348`) reworded it without consulting the session, and the handler's 401 gate (`src/ops/list_accounts.rs:173-180`) never ran. Expected per the crate's own rule at `src/service.rs:363-370` and `tests/all/portal_session.rs:801-802` ("A request presenting no usable session never gets this far: it is 401, whatever else it carries"): 401 `UnauthorizedException` with `"Session token not found or invalid"`.
4. Show the inconsistency inside one operation: dispatch `GET https://localhost:9297/assignment/accounts?next_token=not-a-cursor` with the SAME unusable token. Observed: 401 `UnauthorizedException` (pinned by `tests/all/bearer_token_gate.rs:113-117`). Two equally unreadable parameters of the same operation get opposite precedences.
5. Repeat step 2 against the other listing: `GET /assignment/roles?account_id=111111111111&max_result=0` with the same unusable token — also 400, since `max_result` is modeled on `ListAccountRoles` too and the same seam answers it.
6. Repeat step 2 with `tok-logged-out` (logged out via `POST /logout`) and with a session seeded `Duration::ZERO` as `tests/all/bearer_token_gate.rs:223-229` does: both classes of unusable session are likewise answered 400 instead of the gate's one 401.
