Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 1c2532f976759109af85971b1feb487be7787ac2

# Both Support App listings refuse a nextToken with the synthetic framework ValidationException instead of the service's own modeled one

_Filed: 2026-10-29T06:46:38Z_
_Operation: ListSlackChannelConfigurations | Class: error-response_

> _Note: this issue was identified against the code base at commit `1c2532f976759109af85971b1feb487be7787ac2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSlackChannelConfigurations` and `ListSlackWorkspaceConfigurations` refuse a request that carries a `nextToken` through `crate::scaffolded::Scaffolded::validation(..)`, which renders the SYNTHETIC `smithy.framework#ValidationException` shape (declared inside `src/scaffolded.rs`, deliberately outside any service model). This crate's own scaffold documentation states the rule for that channel twice, and both statements exclude this case: the wrapper variant is for "a service whose model carries no client-input error", and when the SERVICE models its own `ValidationException` the handler must use THAT shape, widening the operation's error enum with `#[operation_error]`'s `extra_errors` for precisely the situation where the service models the error but the operation does not list it.

This service DOES model its own client-input error: `com.amazonaws.supportapp#ValidationException` (errorKind client, httpError 400), and the five sibling handlers in this same crate all construct that modeled shape. Only the two listings reach for the synthetic one, so the error shape identity a Support App refusal carries differs depending on which operation refused the request, and the crate's documented remedy (`extra_errors`) is left unused.

The refusal's status (400), its wire error code (`ValidationException`) and its `message` are unchanged by the fix — the modeled shape also has only a `message` member — so this is a shape-identity/consistency defect with no response change, not a behavioural one.

## Evidence

Rule, stated in this crate (services/service-com-amazonaws-supportapp-supportapp/src/scaffolded.rs:39-42):
  "An implemented operation stops needing the 501, but the wrapper is also the only place it can return an error its own model does not declare — `validation` above, for a service whose model carries no client-input error."

Same rule, restated on the constructor itself (src/scaffolded.rs:215-220):
  "If this service's model declares its own `ValidationException` (or another client-input error) use THAT: the modeled shape may carry members beyond `message` ... When the operation already lists it, it is a plain variant on the generated error enum; `#[operation_error]`'s `extra_errors` is only for widening in a shape the SERVICE models that this OPERATION does not list."

The synthetic shape it hands back is explicitly not in the model (src/scaffolded.rs:131-144):
  "In the `smithy.framework` namespace for the same reason as `NotImplemented`: the shape is not in any service's model, so it must not collide with a modeled one. If THIS service's model declares its own `ValidationException`, prefer that" — `ShapeId::from_static("smithy.framework#ValidationException", ...)`.

Code that breaks it:
  src/ops/list_slack_channel_configurations.rs:150 — `return Err(crate::scaffolded::Scaffolded::validation(message));`
  src/ops/list_slack_workspace_configurations.rs:155 — `return Err(crate::scaffolded::Scaffolded::validation(message));`

The crate's reasoning for that choice is scoped to the OPERATION, not the service (src/state.rs:309-311):
  "Neither paginated operation models a client-input error, so the refusal travels through the scaffold's 400 `ValidationException` channel" — which is exactly the "a shape the SERVICE models that this OPERATION does not list" case scaffolded.rs:218-220 routes to `extra_errors`.

The service DOES model it: smithy_query(query="shape", shape="ValidationException") on services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json returns `{"shape_id": "com.amazonaws.supportapp#ValidationException", "errorKind": "client", "httpError": 400, "members": {"message": ...}}`, and the sibling handlers construct it directly: create_slack_channel_configuration.rs:135, update_slack_channel_configuration.rs:159, delete_slack_channel_configuration.rs:114, delete_slack_workspace_configuration.rs:102, register_slack_workspace_for_organization.rs:155, put_account_alias.rs:85 all carry `ValidationException(ValidationException)` on the generated error enum and return it (e.g. create_slack_channel_configuration.rs:238-243, register_slack_workspace_for_organization.rs:284-294).

Test coverage that must stay green and is unaffected: tests/all/listing_pagination.rs:350-360 asserts only `status == 400` and `error_type == "ValidationException"` for both listings over seven token forms; tests/all/error_message_templates.rs asserts the message envelope "Your {operation} request isn't valid. ... Try your request again." The modeled shape renders the same 400 and the same `ValidationException` code with the same `message`, so both suites pass unchanged.

## Steps to Reproduce

1. Read services/service-com-amazonaws-supportapp-supportapp/src/scaffolded.rs:215-220 — the crate instructs a handler to use the service's own modeled `ValidationException`, and names `#[operation_error]`'s `extra_errors` as the way to widen an operation that does not list it.
2. Read src/ops/list_slack_channel_configurations.rs:85-90: the generated error enum carries only `AccessDeniedException` and `InternalServerException`, with no `extra_errors` widening.
3. Read src/ops/list_slack_channel_configurations.rs:147-151 and src/ops/list_slack_workspace_configurations.rs:152-156: both refuse a `nextToken` via `crate::scaffolded::Scaffolded::validation(message)`, i.e. the synthetic `smithy.framework#ValidationException` declared at src/scaffolded.rs:136-144.
4. Compare with the five sibling handlers that refuse through the service-modeled shape (e.g. src/ops/create_slack_channel_configuration.rs:238-243, src/ops/register_slack_workspace_for_organization.rs:284-294), proving the crate already has the modeled channel wired everywhere else.
5. POST /control/list-slack-channel-configurations with {"nextToken": "not-a-token"}: the 400 is rendered from the framework shape rather than `com.amazonaws.supportapp#ValidationException` (status, code and message are identical, so tests/all/listing_pagination.rs stays green either way).
