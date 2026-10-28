Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a7f4b46fd13e3042ad499c1371bfc738191ffa75

# Both listings raise the handler 400 refusal through the synthetic scaffold ValidationException instead of the crate's modelled one

_Filed: 2026-10-28T01:56:24Z_
_Operation: ListSlackChannelConfigurations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a7f4b46fd13e3042ad499c1371bfc738191ffa75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate raises its handler-level 400 `ValidationException` refusals two different ways, so the two copies of the same refusal path can drift apart.

Every handler-raised request-validation refusal of this service is worded by the one shared envelope `crate::state::invalid_request_message`. Three handlers carry that message in the crate's MODELLED `ValidationException` (the `com.amazonaws.supportapp#ValidationException` struct generated in `src/ops/types/shared_types.rs`, lifted through the operation's own `...ErrorModeled::ValidationException` variant). The two listing handlers carry the very same message in the SYNTHETIC `smithy.framework#ValidationException` of the scaffold wrapper, via `crate::scaffolded::Scaffolded::validation(..)`.

The crate's own `scaffolded.rs` documents that this is the wrong side of the fork when the SERVICE models the error: it says to use the modelled shape, and names `#[operation_error]`'s `extra_errors` as the mechanism for exactly the situation the two listings are in (the service models `ValidationException`; the operation does not list it). `state.rs` documents the opposite choice for the same refusal. One rule, two implementations, two docs that disagree about which one is right.

The two renderings are byte-identical today — same 400, same `ValidationException` wire code, same single `message` member — so this is a pure maintainability finding: consolidating the listings onto the modelled shape must leave every response and every existing test exactly as it is.

## Evidence

The crate's own instruction, `services/service-com-amazonaws-supportapp-supportapp/src/scaffolded.rs:215-220`:
```
    /// If this service's model declares its own `ValidationException` (or another
    /// client-input error) use THAT: the modeled shape may carry members beyond
    /// `message` — a `reason`, a field list — and a real client reads them. When the
    /// operation already lists it, it is a plain variant on the generated error enum;
    /// `#[operation_error]`'s `extra_errors` is only for widening in a shape the
    /// SERVICE models that this OPERATION does not list.
```
This service DOES model its own `ValidationException` — `src/ops/types/shared_types.rs:32-36`:
```
#[smithy_macros::smithy(shape = "com.amazonaws.supportapp#ValidationException")]
#[derive(Debug, Clone, Default)]
pub struct ValidationException {
    pub message: Option<String>,
}
```
Copy 1 — the modelled shape, `src/ops/register_slack_workspace_for_organization.rs:218-228`:
```
            return Err(
                RegisterSlackWorkspaceForOrganizationErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(crate::state::workspace_quota_exceeded_message(Self::NAME)),
                    },
                )
                .into(),
            );
```
(the same construction appears in `src/ops/create_slack_channel_configuration.rs:238-244` and `src/ops/update_slack_channel_configuration.rs:312-318`).

Copy 2 — the synthetic scaffold shape, `src/ops/list_slack_channel_configurations.rs:147-151`:
```
        if let Some(message) =
            crate::state::pagination_token_refusal(Self::NAME, input.next_token.as_deref())
        {
            return Err(crate::scaffolded::Scaffolded::validation(message));
        }
```
and identically `src/ops/list_slack_workspace_configurations.rs:152-156`.

The competing doc, `src/state.rs:217-219`:
```
/// refusal travels through the scaffold's 400 `ValidationException` channel, worded by
/// [`invalid_pagination_token_message`]; `operation` is the caller's `Operation::NAME`.
```

That the consolidation is response-preserving is stated by `src/scaffolded.rs:248-251`: "Every `OperationError` method delegates to the modeled error, so wrapping an implemented operation's error channel changes NOTHING about how that error is rendered" — and both paths render `shape_name()` as the literal `"ValidationException"` with `http_status()` 400 (`src/scaffolded.rs:273-291`) over a single `message` member.

Test coverage that must stay unchanged: `tests/all/listing_pagination.rs:320-361` (`a_next_token_is_refused_by_both_listings`) asserts `refused.status == 400` and `refused.error_type == "ValidationException"` for both listings over seven token forms; `tests/all/error_message_templates.rs:8-16` pins the shared handler-refusal envelope across the severity rule, the `nextToken` refusal and the workspace-quota refusal; `tests/all/resource_preconditions.rs:321-334` pins the modelled-shape path (400 / `ValidationException`) for the workspace quota.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-supportapp-supportapp/src/scaffolded.rs` and read the `Scaffolded::validation` doc at lines 207-223: it instructs that a service whose model declares its own `ValidationException` must use that modelled shape, and names `extra_errors` for an operation that does not list it.
2. Confirm the service models it: `src/ops/types/shared_types.rs:32-36` generates `com.amazonaws.supportapp#ValidationException`.
3. Observe copy 1: `src/ops/create_slack_channel_configuration.rs:238-244`, `src/ops/update_slack_channel_configuration.rs:312-318` and `src/ops/register_slack_workspace_for_organization.rs:218-228` all carry `crate::state::invalid_request_message`-worded text in that modelled shape.
4. Observe copy 2: `src/ops/list_slack_channel_configurations.rs:150` and `src/ops/list_slack_workspace_configurations.rs:155` carry the same kind of message through `crate::scaffolded::Scaffolded::validation(..)`, i.e. the synthetic `smithy.framework#ValidationException`.
5. Note the two crate docs disagree about which path is correct (`src/scaffolded.rs:215-220` vs `src/state.rs:217-219`), and that the rendered responses are identical, so the listings can be moved onto the modelled shape with `cargo test -p service-com-amazonaws-supportapp-supportapp` unchanged.
