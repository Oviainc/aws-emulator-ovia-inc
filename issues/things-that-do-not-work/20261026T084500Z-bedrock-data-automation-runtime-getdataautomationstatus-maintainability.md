Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 617cb494a3d8f9d308b6478ef47b3df7665967e2

# The "unknown invocation ARN" refusal is built four times, status override and comment included

_Filed: 2026-10-26T08:45:00Z_
_Operation: GetDataAutomationStatus | Class: maintainability_

> _Note: this issue was identified against the code base at commit `617cb494a3d8f9d308b6478ef47b3df7665967e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, for exactly this refusal, that it is single-homed so its readers cannot drift: `INVOCATION_NOT_FOUND_MESSAGE` is documented as "the one message behind `ResourceNotFoundException` for every operation here that resolves an ARN" and "Kept in one place so the four readers cannot drift apart" (src/state.rs:310-318). Only the message string was single-homed, though. The rule it belongs to — resolve the caller-supplied ARN in the store, and when it names no retained invocation answer `ResourceNotFoundException` through `Scaffolded::client_error` (HTTP 400 instead of the shape's advisory `@httpError 404`) carrying that message — is written out four times, character for character, including the two-line explanatory comment, in the four handlers that resolve an ARN: `GetDataAutomationStatus`, `TagResource`, `ListTagsForResource` and `UntagResource`.

The copies can drift exactly the way state.rs says they must not: the per-copy part is the STATUS DECISION (`Scaffolded::client_error` vs `Scaffolded::Modeled`), which `src/scaffolded.rs:199-218` documents as a deliberate, evidence-gated choice ("Use it ONLY for a client error whose recorded status is known to differ from the model"). Changing one handler to `Modeled` — or adding a fifth ARN-keyed reader that forgets the override — makes one operation answer 404 for the same logical failure the other three answer 400 with, and nothing in the crate would couple them. This crate already took the opposite route for the sibling cross-handler rule: the tag-key refusal's scan and wording live once in `crate::state` (`malformed_tag_refusal` / `malformed_tag_key_refusal`, src/state.rs:196-213) precisely "so the operations that accept a tag key cannot come to disagree about the rule" (src/state.rs:183-186).

The four `ResourceNotFoundException` structs are the one generated shape from `super::types` (src/ops/types/shared_types.rs:73-77), so one helper in `crate::state` taking the operation's error-variant constructor collapses the four copies with no change to any response byte: same status (400), same `__type` (`ResourceNotFoundException`), same `message`.

## Evidence

The single-homing intent, stated for this exact refusal — src/state.rs:310-318:
```
/// What this service answers when a caller-supplied ARN names no retained invocation —
/// the one message behind `ResourceNotFoundException` for every operation here that
/// resolves an ARN (`GetDataAutomationStatus`, `TagResource`, `ListTagsForResource`,
/// `UntagResource`).
...
/// Kept in one place so the four readers cannot drift apart.
pub const INVOCATION_NOT_FOUND_MESSAGE: &str = "Data Automation Invocation ARN not found.";
```

Copy 1 — src/ops/get_data_automation_status.rs:186-196:
```
let Some(invocation) = store.invocation_mut(&input.invocation_arn) else {
    // 400, not the 404 the shape's advisory `@httpError` declares: see
    // `crate::scaffolded::Scaffolded::ClientError`.
    return Err(crate::scaffolded::Scaffolded::client_error(
        GetDataAutomationStatusErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message: Some(crate::state::INVOCATION_NOT_FOUND_MESSAGE.to_string()),
            },
        ),
    ));
};
```
Copy 2 — src/ops/list_tags_for_resource.rs:170-180:
```
let Some(invocation) = store.invocation(&input.resource_arn) else {
    // 400, not the 404 the shape's advisory `@httpError` declares: see
    // `crate::scaffolded::Scaffolded::ClientError`.
    return Err(crate::scaffolded::Scaffolded::client_error(
        ListTagsForResourceErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message: Some(crate::state::INVOCATION_NOT_FOUND_MESSAGE.to_string()),
            },
        ),
    ));
};
```
Copy 3 — src/ops/tag_resource.rs:182-190:
```
let Some(invocation) = store.invocation_mut(&input.resource_arn) else {
    // 400, not the 404 the shape's advisory `@httpError` declares: see
    // `crate::scaffolded::Scaffolded::ClientError`.
    return Err(crate::scaffolded::Scaffolded::client_error(
        TagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
            message: Some(crate::state::INVOCATION_NOT_FOUND_MESSAGE.to_string()),
        }),
    ));
};
```
Copy 4 — src/ops/untag_resource.rs:180-188:
```
let Some(invocation) = store.invocation_mut(&input.resource_arn) else {
    // 400, not the 404 the shape's advisory `@httpError` declares: see
    // `crate::scaffolded::Scaffolded::ClientError`.
    return Err(crate::scaffolded::Scaffolded::client_error(
        UntagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
            message: Some(crate::state::INVOCATION_NOT_FOUND_MESSAGE.to_string()),
        }),
    ));
};
```

The decision each copy repeats is the one the crate documents as evidence-gated — src/scaffolded.rs:216-218: "Use it ONLY for a client error whose recorded status is known to differ from the model. An error with no such evidence stays [`Self::Modeled`] and keeps the status its shape declares."

The crate's own precedent for single-homing a cross-handler rule — src/state.rs:183-186: "The scan itself — WHICH keys are looked at, and what a malformed one reads as — lives here once, so the operations that accept a tag key cannot come to disagree about the rule".

The shared shape that makes one helper possible — src/ops/types/shared_types.rs:73-77: `#[smithy_macros::smithy(shape = "com.amazonaws.bedrockdataautomationruntime#ResourceNotFoundException")] ... pub struct ResourceNotFoundException { pub message: Option<String> }`.

Test coverage that pins the behaviour a refactor must not change, for all four operations at once — tests/all/data_automation_invocations.rs:686-750 (`every_arn_keyed_operation_refuses_unknown_and_malformed_arns`): the operation table at lines 692-703 lists `GetDataAutomationStatus`, `ListTagsForResource`, `TagResource`, `UntagResource`, and each unknown ARN asserts `answer.status, 400`, `answer.error_code() == "ResourceNotFoundException"` and `answer.body["message"] == Some("Data Automation Invocation ARN not found.")`.

## Steps to Reproduce

1. Open src/ops/get_data_automation_status.rs:186-196, src/ops/list_tags_for_resource.rs:170-180, src/ops/tag_resource.rs:182-190 and src/ops/untag_resource.rs:180-188 side by side: the same `let Some(invocation) = store.invocation[_mut](..) else { ... Scaffolded::client_error(<Op>ErrorModeled::ResourceNotFoundException(ResourceNotFoundException { message: Some(crate::state::INVOCATION_NOT_FOUND_MESSAGE.to_string()) })) }` block appears four times, with the identical two-line `// 400, not the 404 ...` comment.
2. Read src/state.rs:310-318, which states this refusal is "Kept in one place so the four readers cannot drift apart" — only the message is, not the status override or the shape construction.
3. Simulate the drift the crate warns about: change `Scaffolded::client_error(..)` to `Scaffolded::Modeled(..)` in ONE of the four handlers (e.g. untag_resource.rs:183) and run `cargo test -p service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice`; only that handler's rows of `every_arn_keyed_operation_refuses_unknown_and_malformed_arns` fail (404 instead of 400), proving the four copies are independent.
4. Confirm a single-homed fix is response-identical: add one helper in src/state.rs that takes the operation's `ResourceNotFoundException` variant constructor and returns `crate::scaffolded::Scaffolded::client_error(..)` with `INVOCATION_NOT_FOUND_MESSAGE`, call it from all four handlers, and re-run the test module — tests/all/data_automation_invocations.rs:686-750 still passes unchanged.
