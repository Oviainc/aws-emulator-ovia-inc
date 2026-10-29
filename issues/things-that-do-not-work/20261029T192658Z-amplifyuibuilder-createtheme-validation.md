Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# CreateTheme never applies the crate's name-conflict rule: two live themes of one app environment can share a name

_Filed: 2026-10-29T19:26:58Z_
_Operation: CreateTheme | Class: validation_

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Same single rule, same crate, third resource kind. `crate::state::duplicate_item_name()` is documented as the 409 payload for "a name another live resource of the same app environment already holds", and `CreateComponent` consults the store before taking a name. `CreateTheme` consults nothing: after the client-token replay check and the quota check it mints the record directly, so a second `CreateTheme` naming a theme a live theme of the same (appId, environmentName) already holds answers HTTP 200 with a second theme carrying the same name. `ResourceConflictException` (409) is modelled for `CreateTheme` and this handler can never construct it.

The theme rename path has the same hole: `UpdateTheme` assigns `patch.name` into the record with no lookup of the name's current holder, where `UpdateComponent` refuses that rename with the shared payload. The fix is the theme side of the one rule, create and rename.

Effect: a client keyed on theme names inside an app environment gets silent duplicates instead of the catchable 409 the same request produces for a component.

## Evidence

The rule, stated over RESOURCES:

  services/.../src/state.rs:1148-1152
    /// The message of the conflict refusal of a call that would leave two live
    /// resources of one app environment sharing a name.
  services/.../src/state.rs:1190-1193
    /// The `ResourceConflictException` payload for a name another live resource of
    /// the same app environment already holds.
    pub fn duplicate_item_name() -> ResourceConflictException {

Applied by the sibling create:

  services/.../src/ops/create_component.rs:242-250
    if store
        .component_named(&input.app_id, &input.environment_name, &data.name)
        .is_some()
    {
        return Err(CreateComponentErrorModeled::ResourceConflictException(
            crate::state::duplicate_item_name(),
        )
        .into());
    }

Skipped by the handler under review — the quota check is followed immediately by the mint:

  services/.../src/ops/create_theme.rs:182-203
        if store.total_themes() >= crate::state::THEME_QUOTA { ... }

        let id = store.mint_theme_id(&key);
        let theme = crate::state::StoredTheme { ... name: data.name, ... };

And by the rename:

  services/.../src/ops/update_theme.rs:222-225
        theme.values = patch.values;
        if let Some(name) = patch.name {
            theme.name = name;
        }
  versus services/.../src/ops/update_component.rs:279-289.

`component_named` (services/.../src/state.rs:558-571: "A component name identifies a component within its app environment, so this is what a create (or a rename) consults before taking a name") is the crate's only name lookup; there is no theme counterpart.

Test coverage today: components only (tests/all/component_refusals.rs:205-211; tests/all/resource_error_messages.rs:390-462). No theme test exercises a duplicate name; tests/all/theme_lifecycle.rs:440-467 reuses the single name "Tokened" across rows and asserts 200 for each create, so those rows need distinct names once the check lands.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/themes with themeToCreate = {name: "Studio", values: [{key: "primaryColor", value: {value: "#047d95"}}]} and no clientToken — created (200).
2. POST the IDENTICAL request again with no clientToken.
3. Observed: HTTP 200 and a second theme with a fresh id and name "Studio"; both are readable via GET /app/d1234abcd/environment/staging/themes/{id}, so one app environment holds two live themes sharing a name.
4. Also: PATCH /app/d1234abcd/environment/staging/themes/{second id} with {"values": [...], "name": "Studio"} succeeds even while another live theme holds that name, where the component equivalent answers 409 {"message":"Item name already exists"}.
