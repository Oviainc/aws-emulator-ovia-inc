Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# CreateForm never applies the crate's name-conflict rule: two live forms of one app environment can share a name

_Filed: 2026-10-29T19:26:58Z_
_Operation: CreateForm | Class: validation_

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate implements ONE name-conflict rule, declared over resources, and applies it to exactly one of its three resource kinds.

`crate::state` owns the shared 409 payload `duplicate_item_name()` and documents it as the refusal of "a call that would leave two live resources of one app environment sharing a name". `CreateComponent` consults the store before taking a name and answers that payload; `CreateForm` never looks at the stored names at all — it goes from the client-token replay check straight to the quota check and then mints the record, so a second `CreateForm` naming a form a live form of the same (appId, environmentName) already holds answers HTTP 200 with a second form carrying the same name. `ResourceConflictException` (409) is modelled for `CreateForm` and this handler can never construct it: a grep for `duplicate_item_name` over the crate matches only `create_component.rs` and `update_component.rs`.

The same rule is missing from the form rename path (`UpdateForm` merges `patch.name` into the record with no lookup of the name's current holder, where `UpdateComponent` refuses the rename with the same payload), so the fix is the form side of the one rule, create and rename.

Effect: a client that relies on name uniqueness within an app environment (create-if-absent, name-keyed lookup, idempotent provisioning) silently accumulates duplicate forms and its 409 branch never runs — for components the same request is refused.

## Evidence

The rule, stated over RESOURCES, in this crate's state module:

  services/.../src/state.rs:1148-1152
    /// The message of the conflict refusal of a call that would leave two live
    /// resources of one app environment sharing a name.
    ...
    pub const DUPLICATE_ITEM_NAME_MESSAGE: &str = "Item name already exists";
  services/.../src/state.rs:1190-1196
    /// The `ResourceConflictException` payload for a name another live resource of
    /// the same app environment already holds.
    pub fn duplicate_item_name() -> ResourceConflictException { ... }

The sibling handler that APPLIES it:

  services/.../src/ops/create_component.rs:236-250
    // A component name identifies a component within its app environment, so
    // a name a live component there already holds is a conflict with that
    // existing resource rather than a malformed request:
    // `ResourceConflictException` is what the model declares for it.
    if store
        .component_named(&input.app_id, &input.environment_name, &data.name)
        .is_some()
    { return Err(... ResourceConflictException(crate::state::duplicate_item_name()) ...) }

The handler under review, which SKIPS it — nothing between the quota check and the mint consults the stored names:

  services/.../src/ops/create_form.rs:181-194
        if store.total_forms() >= crate::state::FORM_QUOTA { ... }

        let id = store.mint_form_id(&key);
        let form = crate::state::StoredForm { ... name: data.name, ... };

The rename side of the same gap:

  services/.../src/ops/update_form.rs:201-203
        if let Some(name) = patch.name {
            form.name = name;
        }
  versus services/.../src/ops/update_component.rs:273-289 ("a rename onto a name ANOTHER live component there holds is refused with the same `ResourceConflictException` a duplicate create is").

There is no form_named/theme_named helper in state.rs — `component_named` (services/.../src/state.rs:558-571) is the only one — so no path other than the component's can take the check.

Test coverage today: the rule is pinned for components only — tests/all/component_refusals.rs:205-211 (`a_component_name_is_unique_within_its_app_environment`, "Rule 1: a component name is unique among the live components of one app environment") and tests/all/resource_error_messages.rs:390-462 (`a_duplicate_name_answers_one_fixed_literal`, rows CreateComponent + UpdateComponent only, under a module doc whose table reads "| a name a live resource already holds | 409 | \"Item name already exists\" |" and "the literal is per CLASS, not per operation or resource kind"). No form test exercises a duplicate name; note that the token tests reuse one name and expect 200 (tests/all/form_lifecycle.rs:437 creates "Idempotent" once per row, and :453-458 creates "Untokened" twice), so those rows have to be given distinct names when the check lands.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/forms with formToCreate = {name: "Contact", dataType: {...}, formActionType: "create", fields: {}, style: {}, sectionalElements: {}, schemaVersion: "1"} and no clientToken — the form is created (200).
2. POST the IDENTICAL request again with no clientToken (so the idempotency replay path is not taken).
3. Observed: HTTP 200 and a second form with a fresh id and name "Contact"; GET /app/d1234abcd/environment/staging/forms/{either id} reports two live forms sharing one name in one app environment.
4. Compare with the component path: POST /app/d1234abcd/environment/staging/components twice with the same componentToCreate.name — the second call is refused 409 with {"message":"Item name already exists"} from crate::state::duplicate_item_name().
