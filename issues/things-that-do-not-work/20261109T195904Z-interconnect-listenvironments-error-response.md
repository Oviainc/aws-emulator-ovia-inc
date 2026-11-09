Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: d7c278110cdb86b495c40631d820455c8c9bbafe

# The three pagination-cursor refusals are worded without the service refusal envelope the crate says every rejection carries

_Filed: 2026-11-09T19:59:04Z_
_Operation: ListEnvironments | Class: error-response_

> _Note: this issue was identified against the code base at commit `d7c278110cdb86b495c40631d820455c8c9bbafe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate words its own client-facing refusals through one constant, `crate::state::REFUSAL_ENVELOPE` ("Unable to process request: "), whose doc states the rule categorically and says the refusals are built from that one constant rather than each spelling the prefix out. Exactly one site documents itself as an exception (`arn_scope_violation`, which says it deliberately carries neither the envelope nor the ARN text).

The three list operations' `nextToken` refusals are inline string literals that skip the envelope and give no reason for doing so:

1. `ListEnvironments` — src/ops/list_environments.rs:213 ("nextToken is not a pagination token issued by ListEnvironments");
2. `ListConnections` — src/ops/list_connections.rs:250;
3. `ListAttachPoints` — src/ops/list_attach_points.rs:182.

The divergence is visible inside ONE handler: `ListEnvironments` answers its `provider` refusal through `crate::state::provider_violation`, which carries the envelope, and its `nextToken` refusal — the same error shape (`InterconnectValidationException`), the same HTTP 400, the same operation — without it. A client that strips or matches on the documented prefix therefore sees two message shapes from one operation. This is the same rule the already-filed CreateConnection envelope issue (20261108T175135Z) proves, at three sites that issue does not name and on operations it does not cover.

## Evidence

The rule, stated by the constant itself — services/service-com-amazonaws-interconnect-interconnect/src/state.rs:1106-1112:
```
/// The envelope this service puts on the client-facing refusals it words itself.
///
/// Every recorded Interconnect rejection message starts with it ("Unable to process
/// request: This environment model with id … could not be found", "Unable to process
/// request: Connection does not exist for …"), so the refusals below are built from
/// this one constant rather than each spelling the prefix out.
pub const REFUSAL_ENVELOPE: &str = "Unable to process request: ";
```
The ONE site that documents omitting it — src/state.rs:1325-1328:
```
/// request carried the offending ARN. It carries neither this service's
/// [`REFUSAL_ENVELOPE`] nor the ARN text, so a refusal reproduces nothing of the ARN
/// the caller supplied.
```
The envelope-less cursor refusal — src/ops/list_environments.rs:206-218:
```
        let cursor = match input.next_token.as_deref() {
            Some(token) => {
                let key = crate::state::decode_cursor(token, crate::state::ENVIRONMENT_CURSOR_TAG)
                    .filter(|key| crate::state::environment_in_region(&region, key).is_some());
                let Some(key) = key else {
                    return Err(ListEnvironmentsErrorModeled::InterconnectValidationException(
                        InterconnectValidationException {
                            message: "nextToken is not a pagination token issued by ListEnvironments"
                                .to_string(),
                        },
                    )
                    .into());
                };
```
and the sibling refusal of the SAME operation, which does carry it — src/ops/list_environments.rs:155-164:
```
        if let Some(message) = input
            .provider
            .as_ref()
            .and_then(crate::state::provider_violation)
        {
            return Err(ListEnvironmentsErrorModeled::InterconnectValidationException(
                InterconnectValidationException { message },
            )
            .into());
        }
```
with src/state.rs:660-664:
```
    (length > PROVIDER_NAME_MAX_LENGTH).then(|| {
        format!(
            "{REFUSAL_ENVELOPE}provider.{member} must be at most {PROVIDER_NAME_MAX_LENGTH} characters long, and one of length {length} is not"
        )
    })
```
The same literal at the other two sites — src/ops/list_connections.rs:243-255:
```
                    return Err(ListConnectionsErrorModeled::InterconnectValidationException(
                        InterconnectValidationException {
                            message: "nextToken is not a pagination token issued by ListConnections"
                                .to_string(),
```
and src/ops/list_attach_points.rs:179-187:
```
                    return Err(ListAttachPointsErrorModeled::InterconnectValidationException(
                        InterconnectValidationException {
                            message:
                                "nextToken is not a pagination token issued by ListAttachPoints"
                                    .to_string(),
```
Test coverage: the cursor refusals are exercised but only on status and code, never on message text — tests/all/connection_lifecycle.rs:288-299 (`ListEnvironments`, asserts HTTP 400 and `__type` contains `InterconnectValidationException`) and tests/all/connection_lifecycle.rs:1037-1045 (`ListConnections`, asserts HTTP 400) — while the sibling refusal of the same operation has its envelope pinned verbatim in tests/all/provider_member_constraints.rs:41,74 (`const REFUSAL_ENVELOPE: &str = "Unable to process request: ";` … `"{REFUSAL_ENVELOPE}provider.{member} must be at most …"`). So no existing test pins the absence of the envelope on these three messages as intended.

## Steps to Reproduce

1. Boot the emulator and point an Interconnect client at it.
2. Call ListEnvironments with a provider whose member value is longer than 32 characters, e.g. {"provider": {"cloudServiceProvider": "<33 chars>"}}. Observe InterconnectValidationException (HTTP 400) whose message starts with "Unable to process request: provider.cloudServiceProvider must be at most 32 characters long, …".
3. Call ListEnvironments with {"nextToken": "not-a-cursor"}. Observe the SAME error shape and status, but the message "nextToken is not a pagination token issued by ListEnvironments" — no "Unable to process request: " prefix.
4. Repeat step 3 against ListConnections ({"nextToken": "not-a-cursor"}) and ListAttachPoints ({"environmentId": "<an id ListEnvironments returned>", "nextToken": "not-a-cursor"}): both answer the same envelope-less sentence.
5. Compare the two message shapes against src/state.rs:1106-1112, which says every rejection this service words starts with the envelope, and against src/state.rs:1325-1328, the only site that documents omitting it.
