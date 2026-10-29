Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0a6367a2f72726ece298ef6fb266120e608ee126

# ListDomains refuses a bad request with the framework's synthetic smithy.framework#ValidationException while answering its server failure with the service-modelled shape, the opposite of the rule scaffolded.rs states

_Filed: 2026-10-29T11:06:43Z_
_Operation: ListDomains | Class: error-response_

> _Note: this issue was identified against the code base at commit `0a6367a2f72726ece298ef6fb266120e608ee126`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDomains` declares no errors of its own (`errors: []`). The handler meets two error classes, and resolves the identical situation — "an error shape this operation's own `errors` list omits but this SERVICE models and six of its operations declare" — in two opposite ways:

* for the SERVER failure (unreachable store) it widens the service-modelled `com.amazonaws.connecthealth#InternalServerException` in through `#[operation_error(.., extra_errors = ["InternalServerException"])]` and constructs `ListDomainsErrorModeled::InternalServerException`;
* for the CLIENT refusals (a `maxResults` outside the bound, a `nextToken` this service never issued, a `status` outside the enum) it constructs `crate::scaffolded::Scaffolded::validation(..)`, which is the FRAMEWORK's synthetic `smithy.framework#ValidationException` — even though `com.amazonaws.connecthealth#ValidationException` is modelled by this service (errorKind client, httpError 400) and declared by six of its operations, so it could be widened in by the very same `extra_errors` mechanism the same attribute already uses.

`src/scaffolded.rs` states the rule the second branch breaks: when the service's model declares its own `ValidationException`, use THAT, and `extra_errors` "is only for widening in a shape the SERVICE models that this OPERATION does not list" — which is exactly this case.

Sibling handlers of this crate facing the same decision do widen the modelled shape in: `list_subscriptions.rs` answers the identical `Page::resolve` refusal with `ListSubscriptionsErrorModeled::ValidationException`, `start_patient_insights_job.rs` answers its job-count refusal with `StartPatientInsightsJobErrorModeled::ValidationException`, and `create_domain.rs` answers the account cap with `CreateDomainErrorModeled::ValidationException` (widened in through `extra_errors`) — while the SAME `create_domain.rs` handler answers its tag-entry refusal through the synthetic shape, so one handler emits two different shapes under one error code. The other synthetic-shape sites are `tag_resource.rs:221` and `untag_resource.rs:211`.

The crate's own test helpers call what these paths answer "the service's `ValidationException`", which is not the shape that is constructed.

Behavioural impact is nil on restJson1 today — `shape_name()` yields the literal `"ValidationException"` for both variants and both shapes carry exactly one `message` member — which is why severity is LOW. The defect is the split itself: two shapes stand behind one documented failure mode of one service, so the two paths can only be kept in agreement by hand, exactly the drift that was already filed and fixed once for this crate's server-side channel (`connecthealth-createdomain-error-response--r2`, where 7 handlers answered an unreachable store as a 400 client refusal and 8 as the modelled 500).

## Evidence

The rule, in this crate — services/service-com-amazonaws-connecthealth-connecthealth/src/scaffolded.rs:215-220:
```
    /// If this service's model declares its own `ValidationException` (or another
    /// client-input error) use THAT: the modeled shape may carry members beyond
    /// `message` — a `reason`, a field list — and a real client reads them. When the
    /// operation already lists it, it is a plain variant on the generated error enum;
    /// `#[operation_error]`'s `extra_errors` is only for widening in a shape the
    /// SERVICE models that this OPERATION does not list.
```

The service DOES model it — `smithy_query(query="shape", shape="ValidationException")`:
```
"ValidationException": {"shape_id": "com.amazonaws.connecthealth#ValidationException", "type": "structure", "errorKind": "client", "httpError": 400, "members": {"message": {"target": "String"}}, "doc": "<p>This error is thrown when the client supplies invalid input to the API.</p>"}
```

Side A — the same handler widens the service-modelled server shape in, with precisely the justification that also covers `ValidationException` — src/ops/list_domains.rs:89-103:
```
// `InternalServerException` is widened in through `extra_errors`: this operation's own
// `errors` list is EMPTY, but the shape is one this SERVICE models
// (`smithy_query(query="shape", shape="InternalServerException")`, errorKind server,
// httpError 500, retryable) and six of its operations declare — and a server-side
// failure has to be reported as one.
#[smithy_macros::operation_error(
    "com.amazonaws.connecthealth#ListDomains",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum ListDomainsErrorModeled {
    /// Declared on the service (and on six of its operations), not on this one: the
    /// store this listing enumerates could not be reached.
    InternalServerException(InternalServerException),
}
```

Side B — the client refusals of the very same handler use the framework's synthetic shape instead — src/ops/list_domains.rs:182-186 and 207-209:
```
                None => {
                    return Err(crate::scaffolded::Scaffolded::validation(format!(
                        "status '{value}' is not one of {}",
                        DOMAIN_STATUS_VALUES.join(", ")
                    )))
                }
```
```
            Ok(page) => page,
            Err(detail) => return Err(crate::scaffolded::Scaffolded::validation(detail)),
        };
```
and what that constructs — src/scaffolded.rs:130-144:
```
/// Schema for the synthetic `ValidationException` error shape.
///
/// In the `smithy.framework` namespace for the same reason as `NotImplemented`: the
/// shape is not in any service's model, so it must not collide with a modeled one.
/// If THIS service's model declares its own `ValidationException`, prefer that —
/// see [`Scaffolded::validation`].
pub static VALIDATION_EXCEPTION_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#ValidationException",
        "smithy.framework",
        "ValidationException",
    ),
```

The sibling that answers the IDENTICAL `Page::resolve` refusal with the modelled shape — src/ops/list_subscriptions.rs:161-175:
```
        let page = match crate::state::Page::resolve(
            crate::state::subscriptions_page_scope(&region, &input.domain_id),
            input.max_results,
            input.next_token.as_deref(),
        ) {
            Ok(page) => page,
            Err(detail) => {
                return Err(ListSubscriptionsErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(detail),
                    },
                )
                .into())
            }
        };
```

The same split INSIDE one handler — src/ops/create_domain.rs:295-297 (synthetic) versus src/ops/create_domain.rs:334-344 (modelled, widened in through the same `extra_errors` at src/ops/create_domain.rs:210-213):
```
        let requested_tags =
            crate::service::tag_bounds::accepted_entries(input.tags.unwrap_or_default())
                .map_err(crate::scaffolded::Scaffolded::validation)?;
```
```
        if held >= crate::state::MAX_DOMAINS_PER_ACCOUNT {
            return Err(CreateDomainErrorModeled::ValidationException(
                ValidationException {
                    message: Some(format!(
                        "Account {account_id} has reached the maximum limit of {} domains.",
```
The other synthetic sites: src/ops/tag_resource.rs:220-221 (`.map_err(crate::scaffolded::Scaffolded::validation)?`) and src/ops/untag_resource.rs:208-212 (`return Err(crate::scaffolded::Scaffolded::validation(violation));`).

What the suite believes it is asserting — tests/all/tag_map_bounds.rs:110-113:
```
/// A refusal of a tag entry: 400, the service's `ValidationException`, and a message
/// naming the length constraint that was violated. The text itself is not the
/// requirement — the status, the code and the fact that the sentence states the violated
/// constraint are.
```
and tests/all/reserved_tag_key_prefix.rs:68-71:
```
/// A refusal of a reserved key: 400, the service's `ValidationException`, and a message
/// that names the key the caller asked for and the prefix no caller may use.
```

Test coverage: every suite that exercises these paths asserts only the HTTP status and the `error_code` string, never which shape produced it — tests/all/shared_pagination_policy.rs:241 and :392, tests/all/pagination_token_issuance.rs:145, tests/all/list_domains_filter_cursor_scope.rs:168 and :257, tests/all/list_domains_account_scope.rs:281 and :335, tests/all/tag_map_bounds.rs:122-126, tests/all/reserved_tag_key_prefix.rs:77-81 — so switching ListDomains (and the three tag paths) to the service-modelled `ValidationException` leaves every response byte and every existing assertion unchanged, which is what makes the two shapes free to drift unnoticed.

## Steps to Reproduce

1. Read src/scaffolded.rs:215-220: the crate states that when this service's model declares its own ValidationException, a handler must use THAT shape, and that `extra_errors` exists precisely to widen in a shape the SERVICE models but the OPERATION does not list.
2. Read src/ops/list_domains.rs:89-103: the handler already applies that mechanism, widening in the service-modelled `InternalServerException` because ListDomains' own `errors` list is empty.
3. Read src/ops/list_domains.rs:182-186 and 207-209: for its client refusals the SAME handler constructs `crate::scaffolded::Scaffolded::validation(..)`, i.e. the synthetic `smithy.framework#ValidationException` of src/scaffolded.rs:130-144, instead of widening in `com.amazonaws.connecthealth#ValidationException` the same way.
4. Compare src/ops/list_subscriptions.rs:161-175, which answers the byte-identical `Page::resolve` refusal with the service-modelled `ValidationException`, and src/ops/create_domain.rs:295-297 versus :334-344, where one handler emits both shapes under one error code.
5. Send GET /domain?maxResults=101 and GET /domain?nextToken=00 and observe 400 / `ValidationException`; send GET /domains/{domainId}/subscriptions?maxResults=101 and observe the same 400 / `ValidationException` — one documented failure mode of one service answered from two different error shapes, with nothing in the suite pinning which.
