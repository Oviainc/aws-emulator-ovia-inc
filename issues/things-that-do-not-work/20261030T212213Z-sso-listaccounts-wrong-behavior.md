Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6

# This service's path matcher is stricter than the REST router it claims to mirror, so a slash-padded bound route is served on one endpoint, 404 UnknownService on another, and escapes the 401 gate

_Filed: 2026-10-30T21:22:13Z_
_Operation: ListAccounts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`service.rs` recognises this service's own four routes twice by hand — in `serves_route` (which decides whether the service CLAIMS an otherwise unattributed request) and in `absent_required_members` (which decides whether `override_routing_error` answers the portal's 401/400 wording). Both compare the request path to a `ROUTES` literal with `==`, and `serves_route`'s doc asserts that this is "the same (method, path) pair `restJson1` routes on". It is not the same test: the REST router both protocols share normalises the path with `path.trim_matches('/')` and parses the `@http` pattern with the same trim, so `GET /assignment/accounts/` (or `//assignment/accounts`) matches the modelled ListAccounts route in the framework's table while it matches NOTHING in this crate's two hand-rolled tables.

That disagreement produces three wrong answers for one request shape the model binds:

1. Endpoint-dependent service. Addressed to the modelled portal host, `GET /assignment/accounts/` is claimed (the host-style route key is `portal`, so `can_handle`'s `Some(key)` branch claims it without consulting `serves_route`), the framework trims the slash, and the request is SERVED 200 with the account list. Addressed to an endpoint that names no service — `localhost:9297`, the emulator's own endpoint and the normal case — `resolve_route_key` yields `None`, `can_handle` falls through to `serves_route`, the exact comparison fails, and the identical request is answered 404 `UnknownService`. The crate states the opposite rule for itself: each of the four operations must answer "its modelled result … whatever endpoint host the client was pointed at, including none at all".

2. The authentication gate is bypassed. On a claimed endpoint, the slash-padded request with NO bearer header is rejected by the framework's route lookup (the `@required` header is absent), so it reaches `override_routing_error`; there `absent_required_members` exact-matches nothing, returns `None`, and the `?` makes the whole seam return `None` BEFORE `presents_a_usable_session` is ever asked. The caller gets the framework's routing answer (501 `NotImplemented`, the status `tests/all/portal_session.rs:778-785` pins for a claimed-but-unroutable path) instead of the 401 `UnauthorizedException` that `ROUTES`' own doc and `tests/all/bearer_token_gate.rs` state is answered "on every bound route, whatever else the request carries or omits".

3. The same miss removes the modelled 400 wording: a slash-padded request from an authenticated caller that omits `account_id` also falls back to 501 `NotImplemented` instead of the `InvalidRequestException` the seam exists to produce.

The defect is the second, stricter copy of a rule the framework already implements — not the absence of a feature: the crate already matches paths here and documents the match as identical to the router's.

## Evidence

The crate's claim test and the rule it states for itself (`services/service-com-amazonaws-sso-swbportalservice/src/service.rs:183-201`):
```rust
/// Whether `parts` addresses one of this service's own modelled routes: its method and
/// its path are an `@http` binding in [`ROUTES`].
///
/// This is the request's own SIGNATURE — the same (method, path) pair `restJson1` routes
/// on — and for a portal request it is the only service-identifying signal there is.
...
/// Matching is EXACT on both halves, never a prefix: a path this model does not bind
/// stays unclaimed (the router's own `UnknownService`) rather than being swallowed here.
fn serves_route(parts: &http::request::Parts) -> bool {
    let path = parts.uri.path();
    ROUTES
        .iter()
        .any(|(method, route, _)| parts.method == *method && *route == path)
}
```

The matcher it claims to mirror does NOT compare paths exactly. `restJson1` routes through `route_lookup` (`vendor/smithy-schema-server/src/rest_json.rs:154-163`):
```rust
    fn routing_key<'a>(&self, request: &'a Request) -> Result<&'a str, ProtocolError> {
        let uri = request.uri();
        let path = request_path(uri);
        ...
        route_lookup(&self.routes, method, path, query, &|name| {
            request.headers().get(name).is_some()
        })
```
and `route_lookup` normalises both sides (`vendor/smithy-schema-server/src/rest_xml.rs:388-393`):
```rust
    let path_trimmed = path.trim_matches('/');
    let path_segments: Vec<&str> = if path_trimmed.is_empty() {
```
with the pattern trimmed identically (`vendor/smithy-schema-server/src/rest_xml.rs:126-130`):
```rust
fn parse_pattern(pattern: &str) -> Vec<Segment<'_>> {
    let trimmed = pattern.trim_matches('/');
```
So `/assignment/accounts/` and `//assignment/accounts` both resolve to the modelled `/assignment/accounts` route in the framework's table, and to nothing in `ROUTES` (`src/service.rs:139-148`).

The second hand-rolled copy, which gates the service's own refusal wording (`src/service.rs:205-221`):
```rust
fn absent_required_members(req: &aws_smithy_runtime_api::http::Request) -> Option<Vec<String>> {
    let uri = req.uri();
    let path = request_path(uri);
    let (.., required) = ROUTES
        .iter()
        .find(|(method, route, _)| *method == req.method() && *route == path)?;
```
and the seam that depends on it, where the `?` short-circuits BEFORE the credential gate (`src/service.rs:354-373`):
```rust
        let absent = absent_required_members(req)?;
        ...
        if !self.presents_a_usable_session(req) {
            return crate::state::unauthorized_response(self.entry.protocol());
        }
```

The rule that short-circuit breaks (`src/service.rs:119-129`):
```
/// `restJson1` selects a route only when every one of those members is on the request,
/// so a request that omits one matches no operation at all — including the one that
/// omits the bearer credential, which is why the portal's authentication gate has to be
/// answered in [`Service::override_routing_error`] as well as in every handler.
```
and (`tests/all/bearer_token_gate.rs:6-13`):
```
//! RULE: the portal authenticates every request by the sign-in session its bearer token
//! names ... a request that presents no usable session — the modelled header absent, empty, or naming a session
//! that is unknown, logged out or expired — is refused as unauthenticated (HTTP 401
//! `UnauthorizedException`, with the gate's one message) on every bound route, whatever
//! else the request carries or omits.
```

The endpoint-agnostic rule symptom 1 breaks (`tests/all/endpoint_agnostic_routing.rs:5-12`):
```
//! RULE: ... the request's own modelled signature — the (method, path) pair of one of the four
//! `@http` bindings — is what attributes it to this service. So each of the four
//! operations answers its modelled result (its own 200 from state, or 401
//! `UnauthorizedException` for a token that names no usable session) whatever endpoint
//! host the client was pointed at, including none at all.
```

What an unroutable claimed path answers today, pinned (`tests/all/portal_session.rs:778-785`):
```rust
    let unmodeled = portal.get("/assignment/nope", Some("tok-live")).await;
    assert_eq!(
        (unmodeled.status(), error_code(&unmodeled).as_str()),
        (StatusCode::NOT_IMPLEMENTED, "NotImplemented"),
```
and that a request naming no service and no `ROUTES` row is 404 `UnknownService` (`tests/all/endpoint_agnostic_routing.rs:522-556`: the `"a path this model does not bind"` row on `localhost:9297`).

Test coverage: `tests/all/endpoint_agnostic_routing.rs:46-96` sweeps six endpoint hosts × the four routes, but every row spells the path exactly as the model does (`"/assignment/accounts"`, `"/assignment/roles?account_id=…"`, …); no row varies the slash spelling, and neither does `tests/all/portal_session.rs` or `tests/all/bearer_token_gate.rs`. That is why the two hand-rolled matchers can disagree with the framework's without failing the suite.

## Steps to Reproduce

1. Seed the portal as `tests/all/portal_session.rs:85-114` does: `put_account("111111111111", "Production", "production@example.com", ["AdministratorAccess"])` and `put_session("tok-live", …, ["111111111111"], ttl)`, with `EmulatedService` registered on a `MultiServiceRouter`.
2. Dispatch `GET https://portal.sso.us-east-1.amazonaws.com/assignment/accounts/` (note the trailing slash) with `host: portal.sso.us-east-1.amazonaws.com` and `x-amz-sso_bearer_token: tok-live`. Observed: 200 with the seeded `accountList` — `can_handle` claims it by the host (route key `portal`) and `route_lookup` trims the slash onto the modelled route.
3. Dispatch the IDENTICAL request to an endpoint that names no service: `GET https://localhost:9297/assignment/accounts/` with `host: localhost:9297` and the same bearer header. Observed: 404 `UnknownService` — `resolve_route_key` is `None`, so `can_handle` falls to `serves_route`, whose `*route == path` comparison fails on the trailing slash. Expected per `tests/all/endpoint_agnostic_routing.rs:5-12`: the same modelled result on either endpoint.
4. Dispatch `GET https://portal.sso.us-east-1.amazonaws.com/assignment/accounts/` with NO `x-amz-sso_bearer_token` header. Observed: the framework's routing answer (501 `NotImplemented`, as `tests/all/portal_session.rs:778-785` pins for a claimed-but-unroutable path), because `absent_required_members` exact-matches nothing and `override_routing_error` returns `None` before `presents_a_usable_session` runs. Expected per `src/service.rs:119-129` and `tests/all/bearer_token_gate.rs:6-13`: 401 `UnauthorizedException` "Session token not found or invalid".
5. Dispatch `GET https://portal.sso.us-east-1.amazonaws.com/assignment/roles/` with `x-amz-sso_bearer_token: tok-live` and no `account_id`. Observed: the same 501 `NotImplemented` instead of the modelled 400 `InvalidRequestException` the seam produces for the unslashed spelling (`tests/all/endpoint_agnostic_routing.rs:564-608`).
6. Repeat steps 2-5 with a doubled leading slash (`//assignment/accounts`), which `path.trim_matches('/')` normalises identically in the framework and not at all in `ROUTES`.
