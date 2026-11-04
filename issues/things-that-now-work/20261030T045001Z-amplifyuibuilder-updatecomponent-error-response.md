Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: affadad5ad07d45194cd8d4d8bd9c0dc7050195c

# UpdateComponent's module doc states the opposite not-found refusal from the one its handler answers

_Filed: 2026-10-30T04:50:01Z_
_Operation: UpdateComponent | Class: error-response_

> _Note: this issue was identified against the code base at commit `affadad5ad07d45194cd8d4d8bd9c0dc7050195c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`update_component.rs` contradicts itself about which error an `id` that names no component gets.

The file's summary doc (lines 4-8) states the rule as: the operation "declares no `ResourceNotFoundException`, so an `id` that names no component in that app environment is refused with the modelled `InvalidParameterException` (HTTP 400)". Everything else in the same file says the opposite: the error enum widens `ResourceNotFoundException` in through `extra_errors` (line 104-106), the comment above that widening says "an id that names no component is reported missing rather than dressed up as an invalid parameter" (lines 96-101), the Errors section of the same doc header lists "`ResourceNotFoundException` (HTTP 404) — widened in through `extra_errors`" (line 77), and the handler returns exactly that 404 (lines 267-270), reserving `InvalidParameterException` for the wrong-scope case alone (lines 261-266).

The code is the correct side: the 404 is the behaviour an already-FIXED issue introduced for this very operation (`20261022T142124Z-amplifyuibuilder-5-behavioral-divergences-...`, "Cause 4 of 5: UpdateComponent against a never-created id answers HTTP 400 instead of 404"), and the sibling update handlers state the same rule correctly in their own summary docs. So the defect is the stale summary sentence, which is the first thing a maintainer reads and which invites reverting a fixed rule. The fix is to correct lines 6-8 to the 404-for-absent / 400-for-wrong-scope rule the handler implements; no response and no test may change.

## Evidence

The doc rule, in the handler's own summary:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs:6-8
    //! `UpdateComponent` declares no `ResourceNotFoundException`, so an `id` that
    //! names no component in that app environment is refused with the modelled
    //! `InvalidParameterException` (HTTP 400).

The code under it, doing the opposite:

  services/.../src/ops/update_component.rs:257-271
        if store
            .component(&input.app_id, &input.environment_name, &input.id)
            .is_none()
        {
            if store.component_in_other_scope(&input.app_id, &input.environment_name, &input.id) {
                return Err(UpdateComponentErrorModeled::InvalidParameterException(
                    crate::state::item_wrong_scope(),
                )
                .into());
            }
            return Err(UpdateComponentErrorModeled::ResourceNotFoundException(
                crate::state::item_not_found(),
            )
            .into());
        }

The same file contradicting its own summary twice more:

  services/.../src/ops/update_component.rs:96-101
    // `ResourceNotFoundException` is widened in through `extra_errors`: `id` is an
    // `@httpLabel` naming a component that may not exist, ... So an
    // id that names no component is reported missing rather than dressed up as an
    // invalid parameter.
  services/.../src/ops/update_component.rs:77
    //! * `ResourceNotFoundException` (HTTP 404) — widened in through `extra_errors` (see below):
  services/.../src/ops/update_component.rs:250-256
        // ... an id a DIFFERENT app environment owns is a wrong-scope
        // request (the modelled `InvalidParameterException`, 400) and an id that
        // names no component anywhere is absent (`ResourceNotFoundException`,
        // 404 — widened in above).

The sibling update handlers, whose summaries state the rule the way the code implements it:

  services/.../src/ops/update_form.rs:7-10
    //! An id that names no
    //! form anywhere is refused with the modelled `ResourceNotFoundException` (404) and an
    //! id a DIFFERENT app environment owns with the modelled `InvalidParameterException`
    //! (400).
  services/.../src/ops/update_theme.rs:6-9
    //! an id that
    //! names no theme in that app environment answers the modelled `ResourceNotFoundException`
    //! (HTTP 404)

The 404 is also the rule a FIXED issue introduced, so the doc (not the code) is the side to correct:

  issues/20261022T142124Z-amplifyuibuilder-5-behavioral-divergences-duplicate-component-name-accepted.md
    "Cause 4 of 5: UpdateComponent against a never-created id answers HTTP 400 instead of 404."

Test coverage: tests/all/resource_error_messages.rs:16-17 pins the read-path literal "The item specified was not found" for the not-found class, and its UpdateComponent rows run through `update_in` (resource_error_messages.rs:426-441); tests/all/component_refusals.rs:486-488 pins the component refusal table. These stay unchanged by a doc correction.

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs lines 4-8 — the summary says an id naming no component is refused with InvalidParameterException (HTTP 400).
2. Read the same file lines 77, 96-106 and 250-271 — the error enum widens in ResourceNotFoundException and the handler returns it (404) for exactly that case, using InvalidParameterException only for an id another app environment owns.
3. Compare with services/.../src/ops/update_form.rs:7-10 and services/.../src/ops/update_theme.rs:6-9, whose summaries state the 404 rule.
4. Send PATCH /app/d1234abcd/environment/staging/components/11111111-1111-4111-8111-111111111111 with updatedComponent={"name":"Ghost"}: the answer is HTTP 404 {"message":"The item specified was not found"}, not the 400 the summary doc promises.
