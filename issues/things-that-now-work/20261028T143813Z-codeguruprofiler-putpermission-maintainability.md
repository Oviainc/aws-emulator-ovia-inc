Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7c677bf184452d3016cbf464b9a60cef63f91cb5

# The closed-enum actionGroup refusal is spelled out twice (PutPermission and RemovePermission) over two different source lists

_Filed: 2026-10-28T14:38:13Z_
_Operation: PutPermission | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7c677bf184452d3016cbf464b9a60cef63f91cb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two policy-write handlers each carry their own copy of the same rule: "an action group this service cannot name the permissions of is refused with a ValidationException naming the allowed set". The predicate, the error payload and the message text are identical in both files; only the per-operation error constructor differs. This crate's convention is the opposite — every rule that spans operations is stated once in `crate::state` (`validate_interval` for the interval rule, `validate_tags` for the tag rules, `ListCursor` for the pagination rule) and lifted into each operation's own modelled error.

The duplication also straddles TWO sources of truth, which is what makes the copies able to drift into a wrong answer: the predicate asks the ACTIONS catalogue (`crate::state::action_group_actions(..).is_empty()`) while the message enumerates the MODEL enum (`ACTION_GROUP_VALUES`). An action group added to `ACTION_GROUP_VALUES` but not to `action_group_actions` would make both handlers refuse a value the message simultaneously advertises as allowed — and the mistake would have to be found and fixed twice.

The branch is unreachable over HTTP today (the boundary walk refuses a non-enum `actionGroup` first, with the validator's own sentence), so folding both copies into one helper beside `validate_interval` in `src/state.rs` leaves every response byte-identical and every existing test green.

## Evidence

Copy 1 — services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/put_permission.rs:180-195:
```
        // `actionGroup` targets the closed `ActionGroup` enum, so the boundary
        // validator already refuses anything outside it; this refusal keeps the stored
        // policy to action groups whose permissions the service can name, rather than
        // persisting a statement no reader can interpret.
        if crate::state::action_group_actions(&input.action_group).is_empty() {
            return Err(PutPermissionErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "actionGroup must be one of {}, got {}",
                        ACTION_GROUP_VALUES.join(", "),
                        input.action_group
                    ),
                },
            )
            .into());
        }
```

Copy 2 — services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/remove_permission.rs:172-186 (the comment itself names the duplication):
```
        // The same closed-enum refusal `PutPermission` makes: an action group whose
        // permissions the service cannot name is not one it can hold a statement for,
        // so it can name none to remove either.
        if crate::state::action_group_actions(&input.action_group).is_empty() {
            return Err(RemovePermissionErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "actionGroup must be one of {}, got {}",
                        ACTION_GROUP_VALUES.join(", "),
                        input.action_group
                    ),
                },
            )
            .into());
        }
```

The crate's own one-place convention, src/state.rs:404-409:
```
/// Every action group a resource-based policy can hold a statement for: the values the
/// model's `ActionGroup` enum declares, as the generated
/// [`crate::ops::types::ACTION_GROUP_VALUES`] lists them (one, `agentPermissions`) — the
/// handlers refuse anything outside that list, so there is ONE list of them in this
/// crate and the bound below follows the model rather than a copy of it.
pub const MAX_POLICY_STATEMENTS: usize = crate::ops::types::ACTION_GROUP_VALUES.len();
```
and src/state.rs:2052-2054:
```
/// This service's PAGINATION rule, in one place for every operation that reports an
/// already-ordered sequence a page at a time — the way [`validate_interval`] holds its
/// interval rule and [`validate_tags`] its tag rules.
```

The two predicates read a different list from the one their message names, src/state.rs:424-432:
```
pub fn action_group_actions(action_group: &str) -> Vec<String> {
    match action_group {
        ACTION_GROUP_AGENT_PERMISSIONS => vec![
```

The branch is unreachable over HTTP — the boundary walk answers an invalid `actionGroup` with the validator's sentence, pinned by tests/all/refusal_message_rules.rs:402-406:
```
            "ActionGroup, on PutPermission.actionGroup",
...
            "Value 'bogusPermissions' at 'actionGroup' failed to satisfy constraint: \
```

Tests that cover these two handlers and must stay green unchanged: tests/all/permission_writes.rs::a_put_permission_grant_reads_back_through_get_policy, ::a_second_put_permission_replaces_the_principal_set_and_mints_a_new_revision, ::remove_permission_drops_the_statement_and_deletes_the_policy_with_its_last_one, ::a_policy_write_is_refused_outside_the_modelled_input_and_on_an_unknown_group; tests/all/policy_surface.rs (all four tests); tests/all/refusal_message_rules.rs (the ActionGroup row above).

## Steps to Reproduce

1. Open src/ops/put_permission.rs:180-195 and src/ops/remove_permission.rs:172-186 side by side — the predicate (`crate::state::action_group_actions(..).is_empty()`), the `ValidationException` payload and the `"actionGroup must be one of {}, got {}"` message are the same in both; only the error enum differs.
2. Note the two lists the one rule spans: the predicate reads `crate::state::action_group_actions` (src/state.rs:424-432) while the message enumerates `ACTION_GROUP_VALUES`, so the catalogue and the advertised set can disagree and would have to be corrected in both handlers.
3. Confirm the copies are unreachable over HTTP: tests/all/refusal_message_rules.rs:402-406 pins that `actionGroup=bogusPermissions` is already refused by the boundary walk with "Value 'bogusPermissions' at 'actionGroup' failed to satisfy constraint: …", not with either handler's sentence.
4. Move the check and its sentence into one helper in src/state.rs beside `validate_interval`/`validate_tags`, have both handlers lift it into their own modelled `ValidationException`, and re-run tests/all/permission_writes.rs, tests/all/policy_surface.rs and tests/all/refusal_message_rules.rs — every response and every assertion must be unchanged.
