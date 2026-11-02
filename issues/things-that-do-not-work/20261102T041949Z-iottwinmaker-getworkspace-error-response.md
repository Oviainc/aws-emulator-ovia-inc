Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 491e2bc44313ab0206cd2e0d6c6da9a39f419a26

# GetWorkspace's not-found refusal interpolates the 2048-character IdOrArn label without the crate's clamp, so ResourceNotFoundException.message leaves the modelled @length(0..2048)

_Filed: 2026-11-02T04:19:49Z_
_Operation: GetWorkspace | Class: error-response_

> _Note: this issue was identified against the code base at commit `491e2bc44313ab0206cd2e0d6c6da9a39f419a26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetWorkspace` is the ONE operation of this service whose `workspaceId` label targets `IdOrArn` (`@length(min: 1, max: 2048)`) rather than `Id` (`@length(1..128)`) — every other operation's workspace label is bounded at 128 characters. When the label names no workspace this Region's store holds, the handler builds its `ResourceNotFoundException` through the shared `crate::state::workspace_not_found_message`, which prefixes the caller's value with the 33 fixed characters `"Unable to find workspace for id: "`.

`ResourceNotFoundException$message` targets `ErrorMessage`, which is `@length(min: 0, max: 2048)`. So any label above 2015 characters produces a `message` longer than the member admits, and a label of the member's own modelled maximum (2048) produces one of 2081 characters — 33 past the bound.

The handler does not pass the message through `crate::state::clamped_message`, the helper this crate wrote for exactly this case ("an identifier the model bounds as widely as — or more widely than — the member the message travels on renders text past that member's own bound") and whose rule the sibling `DeleteComponentType` states applies "as every other refusal of this service is". The boundary validator does not police output, so the over-long message reaches the wire.

This is a different call path and a different member from the one the open `ListTagsForResource` clamp issue covers: that issue names `resource_not_found_message` on the three tagging operations' `resourceARN` (`TwinMakerArn`) and does not mention `GetWorkspace`, `workspace_not_found_message`, or the `IdOrArn` label — the only non-tagging member in this service wide enough to overrun `ErrorMessage` on its own.

## Evidence

THE MODEL — `smithy_query(query="shape", shape="IdOrArn,GetWorkspaceRequest")`:
```
"IdOrArn": {"shape_id": "com.amazonaws.iottwinmaker#IdOrArn", "type": "string", "length": {"min": 1, "max": 2048}, "pattern": "^[a-zA-Z_0-9][a-zA-Z_\\-0-9]*[a-zA-Z0-9]+$|^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\\/a-zA-Z0-9_-]+$"}
"GetWorkspaceRequest": {... "workspaceId": {"target": "IdOrArn", "httpLabel": true, "required": true, "length": {"min": 1, "max": 2048}, ...}}
```

THE BOUND THE MESSAGE TRAVELS ON — `smithy_query(query="shape", shape="ResourceNotFoundException,ErrorMessage")`:
```
"ResourceNotFoundException": {... "httpError": 404, "members": {"message": {"target": "ErrorMessage", "length": {"min": 0, "max": 2048}}}}
"ErrorMessage": {"shape_id": "com.amazonaws.iottwinmaker#ErrorMessage", "type": "string", "length": {"min": 0, "max": 2048}}
```

THE UNCLAMPED CALL SITE — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_workspace.rs:177-187:
```
        let not_found = || {
            GetWorkspaceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::workspace_not_found_message(
                    &input.workspace_id,
                )),
            })
        };
        let workspace_id = state
            .resolve_workspace_id(&input.workspace_id)
            .ok_or_else(not_found)?;
        let workspace = state.workspaces.get(&workspace_id).ok_or_else(not_found)?;
```

THE BUILDER THAT ADDS 33 FIXED CHARACTERS — src/state.rs:4911-4922:
```
/// The message for a workspace this service does not hold, on a path that LOOKS
/// a workspace UP to serve the request: built from the request, naming the
/// workspace that was looked for.
...
pub fn workspace_not_found_message(workspace_id: &str) -> String {
    format!("Unable to find workspace for id: {workspace_id}")
}
```

THE HELPER THE CRATE WROTE FOR EXACTLY THIS CASE — src/state.rs:4742-4746 and 4754-4767:
```
/// The greatest number of characters the modelled `ErrorMessage` shape — the
/// member every one of this service's modelled refusals states its reason on —
/// admits: `@length(0..2048)`, from
/// `smithy_query(query="shape", shape="ErrorMessage")`.
pub const ERROR_MESSAGE_MAX_CHARS: usize = 2048;
...
/// A message names what the request named, and an identifier the model bounds
/// as widely as — or more widely than — the member the message travels on
/// renders text past that member's own bound: a `ComponentPath` of 2048
/// characters, or a `Name` of 256, stated on a member capped at 256.
pub fn clamped_message(message: String, max_chars: usize) -> String {
```

THE RULE THE CRATE STATES FOR ITSELF, AND THE SIBLINGS THAT OBEY IT — src/ops/delete_component_type.rs:236-249:
```
        // ... The refusal names as many ids as
        // reference it, so it is stated within the bound the member it travels
        // on declares ([`crate::state::clamped_message`]), as every other
        // refusal of this service is.
...
                    message: Some(crate::state::clamped_message(
                        message,
                        crate::state::ERROR_MESSAGE_MAX_CHARS,
                    )),
```
and the sibling not-found builders that DO clamp the very same `workspace_not_found_message` — src/ops/list_components.rs:227-234:
```
        let not_found = |message: String| {
            ListComponentsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::clamped_message(
                    message,
                    crate::state::ERROR_MESSAGE_MAX_CHARS,
                )),
            })
        };
```
src/ops/list_properties.rs:308-316 is the identical construction. `GetWorkspace` — the one caller whose label can actually overrun the bound — does not.

TEST COVERAGE: tests/all/error_message_bounds.rs:27-34 scopes its rows to "WHICH operation reports it (the five operations that accept a `componentPath`)"; `GetWorkspace` is not one of them, and its `ERROR_MESSAGE_MAX` constant (line 48) is never applied to a workspace label. tests/all/not_found_constructions.rs:124 and :187 assert the exact sentence `format!("Unable to find workspace for id: {workspace_id}")` with short ids only, and tests/all/scope_resolution_order.rs:55 does the same — all of which `clamped_message` leaves byte-for-byte unchanged, since it returns the message untouched when it already fits.

## Steps to Reproduce

1. Start the emulator with an empty IoT TwinMaker store (or create an unrelated workspace).
2. Build a label of exactly 2048 characters that `IdOrArn` admits — e.g. 2048 `a` characters, which matches its first alternative `^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$` and sits on the member's `@length` maximum of 2048 (the `Id` bound of 128 does NOT apply here, because this member targets `IdOrArn`).
3. Call GetWorkspace with that value: `GET /workspaces/<2048-char label>`.
4. Observe HTTP 404 `ResourceNotFoundException` whose `message` is 2081 characters — `"Unable to find workspace for id: "` (33 characters) plus the 2048-character label — 33 characters past the `@length(min: 0, max: 2048)` that `ErrorMessage`, the member it travels on, declares.
5. Contrast ListComponents with an over-long `componentPath`: src/ops/list_components.rs:227-234 routes the same kind of not-found reason through `crate::state::clamped_message(message, crate::state::ERROR_MESSAGE_MAX_CHARS)`, so its message stops at 2048.
6. Confirm no regression risk: repeat with a short workspace id and observe the message is byte-for-byte what tests/all/not_found_constructions.rs:124 already asserts, because `clamped_message` returns a message that already fits unchanged (src/state.rs:4768-4771).
