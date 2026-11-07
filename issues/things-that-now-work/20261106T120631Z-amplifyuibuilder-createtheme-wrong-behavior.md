Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: ce65be7ed23a4ac4ad17d64817768d50cbccb885

# The timestamp wire-form suite skips Theme over HTTP because its doc says Theme's operations are still scaffolded 501s, which they are not

_Filed: 2026-11-06T12:06:31Z_
_Operation: CreateTheme | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ce65be7ed23a4ac4ad17d64817768d50cbccb885`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tests/all/component_wire_fidelity.rs` is the suite the crate designates as THE pin for the `@timestampFormat("date-time")` wire form of this service's four timestamp-carrying shapes — `shared_types.rs` names it by path as the thing that "pins the wire form, so it fails if the attribute does not carry it" when the hand-written schemas are eventually deleted.

That suite's own doc states why its `Theme` coverage is schema-only rather than end to end: Theme's operations "are still scaffolded 501s". That premise is false. `CreateTheme`, `GetTheme`, `UpdateTheme`, `DeleteTheme`, `ListThemes` and `ExportThemes` are all implemented and compute their answers from persisted state — none of them carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and none returns 501 — and `tests/all/theme_lifecycle.rs` already drives a `Theme` payload over HTTP and asserts the date-time form on it, which is the very thing the stale sentence says is unreachable.

So the suite that is nominated as the single guard for the timestamp wire contract exercises `Theme$createdAt`/`$modifiedAt` only against the member schemas, on a reason that no longer holds, while the identical end-to-end rows it runs for `Component` are available for `Theme` too. The defect is the false statement and the coverage decision it justifies; a reader of that suite is told Theme cannot be driven over HTTP by this emulator.

## Evidence

The stale premise, in the suite's own module doc — services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/component_wire_fidelity.rs:21-24
//! Case 1 is checked both end to end over HTTP (the `Component` payload of
//! `CreateComponent`/`GetComponent`/`UpdateComponent`) and directly on the member
//! schemas, which is the only reach the shapes whose operations are still scaffolded 501s
//! (`Theme`) have.

The theme operations the sentence calls scaffolded 501s, computing a 200 from persisted state — services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_theme.rs:152-162
        let store = state.lock();
        let Some(theme) = store.theme(&input.app_id, &input.environment_name, &input.id) else {
            return Err(GetThemeErrorModeled::ResourceNotFoundException(
                crate::state::item_not_found(),
            )
            .into());
        };

        Ok(GetThemeOutput {
            theme: Some(theme.to_theme()),
        })

and services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_theme.rs:232-240
        let entity = theme.to_theme();
        store.insert_theme(theme);
        if let Some(token) = input.client_token {
            store.remember_theme_token(key, token, id);
        }

        Ok(CreateThemeOutput {
            entity: Some(entity),
        })

The crate nominating that suite as the one guard for the wire form — services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/types/shared_types.rs:46-52
// REMOVE THIS BLOCK when the vendored codegen learns `smithy.api#timestampFormat`: ...
// Once `vendor/smithy-macros` emits `with_timestamp_format`, delete the five macros and the
// four `schema_and_codec_for!` blocks and put `#[smithy_macros::smithy(shape = ...)]`
// back on the four structs — `tests/all/component_wire_fidelity.rs` pins the wire form,
// so it fails if the attribute does not carry it.

The reach the stale sentence says Theme does not have, already exercised by a sibling suite — services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/tests/all/theme_lifecycle.rs:348-357
            is_date_time(&at(&created.body, "/createdAt")),
            "case {case:?}: createdAt must be an ISO-8601 date-time string, got {:?}",
            at(&created.body, "/createdAt")
        ...
            at(&created.body, "/modifiedAt"),
            at(&created.body, "/createdAt"),
            "case {case:?}: a theme nothing has updated reports modifiedAt at its createdAt, ...

The model trait the suite is pinning, for the members it only checks on the schema — smithy_query(query="shape",shape="Theme"):
  "createdAt": {"target": "Timestamp", "timestampFormat": "date-time", ... "required": true}
  "modifiedAt": {"target": "Timestamp", "timestampFormat": "date-time", ...}

The schema-only rows the suite runs for Theme — tests/all/component_wire_fidelity.rs:351-363 lists ("Theme$createdAt", &Theme::M_CREATED_AT) and ("Theme$modifiedAt", &Theme::M_MODIFIED_AT) beside the Component/CodegenJob entries, while the HTTP cases at lines 203-332 drive only the Component payload of CreateComponent/GetComponent/UpdateComponent.

## Steps to Reproduce

1. Read tests/all/component_wire_fidelity.rs:21-24: it states Theme's operations are "still scaffolded 501s" and that the member schemas are "the only reach" those shapes have.
2. Grep the theme handlers for the scaffold marker: `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/*theme*.rs` returns nothing, and src/ops/get_theme.rs:152-162 / src/ops/create_theme.rs:232-240 build their output from the stored record and answer 200.
3. POST /app/d1234abcd/environment/staging/themes with themeToCreate = {"name":"Base","values":[]} then GET the same id: both answer HTTP 200 with a Theme payload carrying createdAt and modifiedAt — the HTTP reach the sentence says does not exist (tests/all/theme_lifecycle.rs:348-357 already asserts exactly this).
4. Observe the consequence: the suite that shared_types.rs:51 nominates as the single guard for the `@timestampFormat` wire contract never drives Theme$createdAt / Theme$modifiedAt over the wire, even though the same end-to-end rows it runs for Component are now reachable for Theme.
