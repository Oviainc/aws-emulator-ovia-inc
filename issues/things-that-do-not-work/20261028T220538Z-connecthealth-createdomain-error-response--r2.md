Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 40b622ed07403ef350d596bb19a34a2d990a98a6

# An unreachable store is answered 400 ValidationException by 7 handlers and the modelled 500 InternalServerException by the other 8, although state.rs tells every caller to answer it with "a modelled server error"

_Filed: 2026-10-28T22:05:38Z_
_Operation: CreateDomain | Class: error-response_

> _Note: this issue was identified against the code base at commit `40b622ed07403ef350d596bb19a34a2d990a98a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::service_state` is the one door onto this service's store, and its own doc states the rule for the `None` case: "Answer `None` with a modelled server error, never with a fresh engine." Eight of the fifteen handlers obey it and return the service-modelled `InternalServerException` (HTTP 500). The other seven — CreateDomain, GetDomain, DeleteDomain, ListDomains, TagResource, UntagResource and ListTagsForResource — answer the SAME condition with `crate::scaffolded::Scaffolded::validation(..)`, which renders HTTP 400 with the error code `ValidationException` (`src/scaffolded.rs:285-290`, `shape_name` -> "ValidationException", `http_status` -> 400). So a server-side fault is reported to the caller as "your request was invalid", on a status SDK retry classifiers treat as non-retryable, and the detail text "CreateDomain could not reach this service's store" names no member the caller could fix.

Each of the seven justifies the choice with the same sentence — "this operation models no server-side error, so a refusal is the only channel it has" — and that claim is false by this crate's own mechanism. `InternalServerException` is a shape this SERVICE models (six operations declare it), and the crate already widens service-modelled shapes into operations whose own `errors` list omits them, through `#[operation_error(.., extra_errors = [..])]`: CreateDomain itself widens `ValidationException` in that way, TagResource widens `ResourceNotFoundException` and `ServiceQuotaExceededException`, ListTagsForResource and UntagResource widen `ResourceNotFoundException`. The channel the comment says does not exist is the one the same file opens two hundred lines earlier.

`Scaffolded::validation` is also documented for a different purpose: "Reach for this when the request is wrong in a way the model cannot express". An engine the service was mounted on is not a property of the request.

The branch is reached only when this service is mounted on an engine that is not `InMemoryEngine`, so there is no impact on the shipped emulator — hence LOW — but the crate currently states one rule for this condition and implements two.

## Evidence

The rule, at the helper every handler calls — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1579-1582`:
```
/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
```

Side A — the seven handlers that answer a 400 client-validation error. `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_domain.rs:262-269`:
```
        // The store is unreachable only when this service was mounted on a foreign
        // engine; this operation models no server-side error, so a refusal is the only
        // channel it has.
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(crate::scaffolded::Scaffolded::validation(
                "CreateDomain could not reach this service's store",
            ));
        };
```
The same `Scaffolded::validation` answer for the same condition at `get_domain.rs:191-195` ("GetDomain could not reach this service's store", line 193), `delete_domain.rs:315-320` (line 317), `list_domains.rs:133-137` (line 135), `tag_resource.rs:187-191` (line 189), `untag_resource.rs:181-185` (line 183), `list_tags_for_resource.rs:154-158` (line 156).

What that renders as — `services/service-com-amazonaws-connecthealth-connecthealth/src/scaffolded.rs:273-290`:
```
    fn shape_name(&self) -> &'static str {
        match self {
            Self::NotImplemented(_) => "NotImplemented",
            Self::Modeled(e) => e.shape_name(),
            Self::ValidationException(_) => "ValidationException",
...
    fn http_status(&self) -> u16 {
        match self {
            Self::NotImplemented(_) => 501,
            Self::Modeled(e) => e.http_status(),
            Self::ValidationException(_) => 400,
```

Side B — the eight siblings that answer the modelled server error for the identical condition. `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_subscription.rs:155-164`:
```
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateSubscriptionErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "CreateSubscription could not reach this service's store".to_string(),
                    ),
                },
            )
            .into());
        };
```
and `get_medical_scribe_listening_session.rs:162-164`, which spells out the principle:
```
        // The store is unreachable only when this service was mounted on a foreign
        // engine. This operation models `InternalServerException`, so that is the
        // channel for a server-side failure.
```
Likewise `activate_subscription.rs:151`, `deactivate_subscription.rs:150`, `get_subscription.rs:147`, `list_subscriptions.rs:149`, `get_patient_insights_job.rs:189`, `start_patient_insights_job.rs:196`.

Why "a refusal is the only channel it has" is false in this crate — the widening mechanism already in use. `create_domain.rs:195-204`:
```
// `ValidationException` is widened in through `extra_errors`: this SERVICE models the
// shape (`smithy_query(query="shape", shape="ValidationException")`, errorKind client,
// httpError 400) and six of its operations list it, but `CreateDomain`'s own `errors`
// list names only `ServiceQuotaExceededException` — while the account-wide Domain cap is
// surfaced as a `ValidationException`, so the modelled shape is what this handler
// answers with rather than the framework's synthetic one.
#[smithy_macros::operation_error(
    "com.amazonaws.connecthealth#CreateDomain",
    extra_errors = ["ValidationException"]
)]
```
and `list_tags_for_resource.rs:65-68, 89-92`:
```
//! The model lists NONE for this operation
//! (`smithy_query(query="operation",operation="ListTagsForResource")` reports
//! `errors: []`), so the shape below is widened in through `extra_errors`; it is a shape
//! this SERVICE models and other operations declare:
...
#[smithy_macros::operation_error(
    "com.amazonaws.connecthealth#ListTagsForResource",
    extra_errors = ["ResourceNotFoundException"]
)]
```
(`tag_resource.rs:120-123` widens two shapes the same way; `untag_resource.rs:117-120` one.)

What `Scaffolded::validation` is documented for — `src/scaffolded.rs:207-213`:
```
    /// Refuse the request as invalid, with HTTP 400 `ValidationException`.
    ///
    /// Reach for this when the request is wrong in a way the model cannot express,
    /// so the boundary validator passed it: a value outside a rule the service
    /// enforces but does not model, or a combination of members that cannot hold
    /// together.
```

Test coverage: no test in `tests/all/` drives the store-unreachable branch (every suite mounts the service on the in-memory engine through the shared `Emulator` harness in `tests/all/domain_subscription_lifecycle.rs`); `src/service/error_detail.rs:264-296` only asserts that whatever body is produced carries its text in `Message`, for both `ValidationException` and `InternalServerException`, so it pins neither channel to a condition.

## Steps to Reproduce

1. Read `src/state.rs:1581`: the one accessor onto the store instructs every caller to "Answer `None` with a modelled server error".
2. Grep the handlers for that `None` branch: `rg -n "could not reach this service's store" services/service-com-amazonaws-connecthealth-connecthealth/src/ops`.
3. Observe eight handlers construct `<Op>ErrorModeled::InternalServerException` (HTTP 500) and seven — create_domain:267, get_domain:193, delete_domain:317, list_domains:135, tag_resource:189, untag_resource:183, list_tags_for_resource:156 — construct `crate::scaffolded::Scaffolded::validation(..)`.
4. Follow that variant to `src/scaffolded.rs:277` and `:289`: it answers the error code `ValidationException` with HTTP 400, i.e. a client-input refusal for a server-side fault, on a status SDK retry classifiers do not retry.
5. Read the justification at create_domain.rs:262-264 ("this operation models no server-side error, so a refusal is the only channel it has") against create_domain.rs:201-204 and list_tags_for_resource.rs:89-92, where this crate widens a service-modelled error shape into an operation that does not list it via `extra_errors` — the channel the comment says does not exist.
