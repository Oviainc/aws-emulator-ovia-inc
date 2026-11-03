Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# service.rs computes a request's path two different ways — a hand-rolled splitter and http::Uri::path() — for the same ROUTES match

_Filed: 2026-10-29T18:41:32Z_
_Operation: ListAccounts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service.rs` has to answer one question twice — "what is the path portion of this request's URI?" — in order to match the same `ROUTES` table, and it answers it with two independent implementations:

* `serves_route` (used by `can_handle` to claim a request) uses `parts.uri.path()` from the `http` crate.
* `absent_required_members` (used by `override_routing_error` to reword a missing-parameter refusal) uses the hand-rolled `request_path`, a 12-line splitter that scans for the third `/` in an absolute-form URI.

Both feed `ROUTES.iter().find(...)`/`any(...)` on the same four literal routes, so the two spellings must agree for a claimed request to also be reworded; nothing makes them agree, and they can drift (the hand-rolled branch falls back to `"/"` for a URI with fewer than three slashes, where `http::Uri::path()` does not). The hand-rolled span is replaceable by the `http` crate that is already in Cargo.lock and already imported at the top of this very file (`use smithy_server_runtime::{http, …}`): parsing `req.uri()` into an `http::Uri` and taking `.path()` gives both callers one implementation.

The file's own comment already flags the function as a knowing duplication (of a crate-private vendor helper) and asks for it to be reconciled; the reconciliation available without touching `vendor/` is to use the dependency both halves of the file already depend on. This is a refactor only: every response and every existing test stays as it is.

## Evidence

Copy 1 — the hand-rolled splitter, with its own admission of duplication (`services/service-com-amazonaws-sso-swbportalservice/src/service.rs:150-169`):
```rust
/// The path portion of a request URI: no scheme, no authority, no query string.
///
/// DUPLICATION, knowingly: the framework has the same splitter in
/// `vendor/smithy-schema-server/src/rest_xml.rs` (`pub(crate) fn request_path`), which
/// is crate-private and so unreachable from a service crate; … Reconcile the two by
/// making that one public.
fn request_path(uri: &str) -> &str {
    let without_query = uri.split('?').next().unwrap_or(uri);
    if without_query.starts_with('/') {
        return without_query;
    }
    without_query
        .match_indices('/')
        .nth(2)
        .map_or("/", |(index, _)| &without_query[index..])
}
```
used at `src/service.rs:205-210`:
```rust
fn absent_required_members(req: &aws_smithy_runtime_api::http::Request) -> Option<Vec<String>> {
    let uri = req.uri();
    let path = request_path(uri);
    let (.., required) = ROUTES
        .iter()
        .find(|(method, route, _)| *method == req.method() && *route == path)?;
```

Copy 2 — the same computation via the `http` crate, matching the SAME table (`src/service.rs:196-201`):
```rust
fn serves_route(parts: &http::request::Parts) -> bool {
    let path = parts.uri.path();
    ROUTES
        .iter()
        .any(|(method, route, _)| parts.method == *method && *route == path)
}
```

The replacement crate is already a dependency and already imported here (`src/service.rs:43-45`):
```rust
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
```

Tests that pin both callers' behaviour and must stay green unchanged: `tests/all/endpoint_agnostic_routing.rs:483-557` (`a_request_for_another_service_or_an_unmodelled_route_is_not_claimed`, exercising `serves_route`), `tests/all/endpoint_agnostic_routing.rs:565-609` (`an_absent_required_member_is_an_invalid_request_on_any_endpoint`, exercising `request_path`), and `tests/all/portal_session.rs:804-889` (`an_unreadable_request_is_an_invalid_request`).

## Steps to Reproduce

1. Read `services/service-com-amazonaws-sso-swbportalservice/src/service.rs:150-169` (the hand-rolled `request_path`) and `:196-201` (`serves_route`, which uses `parts.uri.path()`): two implementations of "the request's path", both matched against the single `ROUTES` table at `:139-148`.
2. Note `use smithy_server_runtime::{http, …}` at `:43-45` — the `http` crate whose `Uri::path()` the second copy already uses is available to the first.
3. Fold both onto one path computation (parse `req.uri()` as an `http::Uri` and take `.path()`), then run `cargo test -p service-com-amazonaws-sso-swbportalservice`: `endpoint_agnostic_routing.rs` and `portal_session.rs` must pass byte-identically.
