Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e8c5c92d9bc397e4014b473476bd2d32121bdaea

# Theme reads/updates/deletes answer 404 for an id another app environment owns, where every component and form path answers the wrong-scope 400

_Filed: 2026-10-30T03:37:47Z_
_Operation: GetTheme | Class: error-response_

> _Note: this issue was identified against the code base at commit `e8c5c92d9bc397e4014b473476bd2d32121bdaea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate implements ONE rule for the three resources it addresses by (appId, environmentName, id): an id that resolves under a DIFFERENT app environment is a wrong-scope request refused with the modelled `InvalidParameterException` (400, message "The item specified does not belong to that app or environment"), told apart from an id that names no resource anywhere, which is absent (`ResourceNotFoundException`, 404).

Six of the nine handlers addressed that way apply it — `GetComponent`, `UpdateComponent`, `DeleteComponent` via `state::component_in_other_scope`, and `GetForm`, `UpdateForm`, `DeleteForm` via `state::form_in_other_scope`. The three THEME handlers apply none of it: `GetTheme`, `UpdateTheme` and `DeleteTheme` look the id up only inside the addressed app environment and report every miss as absent, so the identical request ("this theme's id, asked for under another app or another environment of the same app") is answered 400 for a component or a form and 404 for a theme. There is no `theme_in_other_scope` helper in `state.rs` at all, while both siblings have one.

The divergence is ADMITTED in this crate, in `delete_theme.rs`, which names the two kinds that classify it differently and defers the theme side to "its own change". That is the gap: the one rule is implemented for two of three resource kinds, and a client that branches on 400-vs-404 to tell "wrong scope" from "gone" gets a different answer depending on which resource kind it addressed, even though the model gives all three the same identifiers (`ThemeResource` identifiers `{id: Uuid, appId: String, environmentName: String}`, identical to `ComponentResource` / `FormResource`).

## Evidence

The admission, in this crate's own handler, naming the sibling paths and deferring the fix:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/delete_theme.rs:185-195
    // An id ANOTHER app environment owns is reported absent here too, which
    // is what `GetTheme` already answers for it (`get_theme.rs`): the three
    // theme operations agree on which themes exist. A component or a form in
    // that position is instead refused as wrong-scope (400, via
    // `component_in_other_scope` / `form_in_other_scope`), so the two
    // resource kinds classify it differently. Themes are left on the
    // reported-absent classification deliberately: it is the one `GetTheme`
    // was written to and whose units pass, no recorded theme unit shows the
    // 400, and moving all three theme operations onto a
    // `theme_in_other_scope` path would change `GetTheme`'s answer — its own
    // change, not this one's.

The rule as the component side states and applies it:

  services/.../src/state.rs:556-561
    /// Whether `id` names a component some OTHER app environment owns.
    ///
    /// A component is addressed by (appId, environmentName, id), so an id that
    /// resolves outside the app environment the request named is a request made
    /// in the wrong scope — told apart here from an id that names no component at
    /// all, which is simply absent.
  services/.../src/ops/get_component.rs:162-167
    if store.component_in_other_scope(&input.app_id, &input.environment_name, &input.id) {
        return Err(GetComponentErrorModeled::InvalidParameterException(
            crate::state::item_wrong_scope(),
        )
        .into());
    }

And as the FORM side adopted it (so themes are the only kind left out) — `services/.../src/state.rs:768-774`: "form at all, which is simply absent. The component counterpart is [`Self::component_in_other_scope`]. pub fn form_in_other_scope(...)", used at `get_form.rs:161`, `update_form.rs:181`, `delete_form.rs:184`.

The theme handlers, which consult nothing outside the addressed scope:

  services/.../src/ops/get_theme.rs:152-158
    let store = state.lock();
    let Some(theme) = store.theme(&input.app_id, &input.environment_name, &input.id) else {
        return Err(GetThemeErrorModeled::ResourceNotFoundException(
            crate::state::item_not_found(),
        )
        .into());
    };
  services/.../src/ops/update_theme.rs:209-214 (same, `theme_mut`), services/.../src/ops/delete_theme.rs:174-199 (same, `remove_theme`, with the delete literal).

`grep in_other_scope src/` matches component_* and form_* only — there is no theme helper.

The model gives the three kinds the same addressing, so nothing in it separates them: `smithy_query(query="shape", shape="ThemeResource")` → `"identifiers": {"id": "Uuid", "appId": "String", "environmentName": "String"}`, identical to `ComponentResource` and `FormResource`.

Test coverage today: the 400 is pinned for components and forms — tests/all/component_refusals.rs:13-17 ("A component is addressed by (appId, environmentName, id), so an id that resolves under ANOTHER app environment is a wrong-scope request (HTTP 400), told apart from an id that names no component anywhere (HTTP 404). Every operation addressed that way — `GetComponent`, `UpdateComponent`, `DeleteComponent` — answers the same two") and tests/all/form_delete.rs / form_update.rs (rows "a name one... another app" asserting the `does not belong to that app or environment` phrase). The 404 is pinned for themes with no justification beyond restating the current behaviour — tests/all/theme_lifecycle.rs:495-515 (rows "another app", "another environment of this app" asserting 404), tests/all/theme_update.rs:380-393, tests/all/theme_deletion.rs:333-355 — so those three theme rows have to move to 400 with `crate::state::WRONG_SCOPE_MESSAGE` when the theme side of the rule lands.

## Steps to Reproduce

1. Mount the service on one engine (as tests/all/theme_lifecycle.rs does) and create a theme: POST /app/d1234abcd/environment/staging/themes with themeToCreate = {"name": "Studio", "values": [{"key": "primaryColor", "value": {"value": "#047d95"}}]} — note the returned id T.
2. Create a component in the same environment: POST /app/d1234abcd/environment/staging/components with a valid componentToCreate — note the returned id C.
3. GET /app/d9999zzzz/environment/staging/components/C (the component's id asked for under another app): answered HTTP 400 with {"message":"The item specified does not belong to that app or environment"} (get_component.rs:162-167).
4. GET /app/d9999zzzz/environment/staging/themes/T (the theme's id asked for under another app, the identical mis-addressing): answered HTTP 404 with {"message":"The item specified was not found"} instead of the wrong-scope 400 (get_theme.rs:152-158).
5. Repeat with PATCH /app/d9999zzzz/environment/staging/themes/T and DELETE /app/d9999zzzz/environment/staging/themes/T — both 404, where the component and form equivalents are 400 (update_component.rs:261, delete_component.rs:183, update_form.rs:181, delete_form.rs:184).
6. Same divergence for the other half of the key: /app/d1234abcd/environment/prod/themes/T.
