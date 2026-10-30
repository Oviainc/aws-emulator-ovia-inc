Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 6b5a94ef1b44099f297a2ea92764072e31901e34

# GetComponent and DeleteComponent summary docs promise 404 for a wrong-scope id their handlers answer 400 for

_Filed: 2026-10-30T18:45:30Z_
_Operation: GetComponent | Class: error-response_

> _Note: this issue was identified against the code base at commit `6b5a94ef1b44099f297a2ea92764072e31901e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`get_component.rs` and `delete_component.rs` each open with a one-sentence rule that is false for one of the two inputs their handler distinguishes.

Both summaries state the refusal as: "An id that names no component in that app environment answers the modelled `ResourceNotFoundException` (HTTP 404)". An id that names no component in the addressed app environment but names one in ANOTHER app environment satisfies that sentence exactly — and the handler answers `InvalidParameterException` (HTTP 400, "The item specified does not belong to that app or environment") for it, not the 404 the summary promises. The in-body comments of the very same handlers state the two-way rule correctly ("Two different failures, told apart by where the id resolves"), and the sibling form handlers state it correctly in their own summaries (`get_form.rs`, `delete_form.rs` both name the 404 branch AND the 400 branch).

The code is the correct side: the 400 for a wrong-scope id on GetComponent/DeleteComponent is pinned by an existing test, and the theme handlers explicitly document the opposite classification as a deliberate per-kind choice. So the defect is the two stale summary sentences — the first thing a maintainer reads, and the sentence that invites "fixing" the handler back onto a 404 a test forbids. The fix is to state the two branches in both summaries, exactly as `get_form.rs`/`delete_form.rs` do; no response and no test changes.

This is the same class of self-contradiction already filed for `update_component.rs` (issue 20261030T045001Z-amplifyuibuilder-updatecomponent-error-response), on the two component handlers that issue does not cover.

## Evidence

The summary rule, in the handler under review:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_component.rs:4-6
    //! Reports the stored component the request names, exactly as the create or the last
    //! update persisted it. An id that names no component in that app environment answers
    //! the modelled `ResourceNotFoundException` (HTTP 404).

The code under it, which answers 400 for an id that names no component in that app environment but one in another:

  services/.../src/ops/get_component.rs:154-172
        let Some(component) = store.component(&input.app_id, &input.environment_name, &input.id)
        else {
            // Two different failures, told apart by where the id resolves: an id
            // a DIFFERENT app environment owns is a request made in the wrong
            // scope — the modelled `InvalidParameterException` (400) ... while an id
            // that names no component anywhere is absent, the modelled
            // `ResourceNotFoundException` (404).
            if store.component_in_other_scope(&input.app_id, &input.environment_name, &input.id) {
                return Err(GetComponentErrorModeled::InvalidParameterException(
                    crate::state::item_wrong_scope(),
                )
                .into());
            }

The second file with the identical defect:

  services/.../src/ops/delete_component.rs:4-6
    //! Removes the stored component the request names and answers an empty 2xx. An id that
    //! names no component in that app environment answers the modelled
    //! `ResourceNotFoundException` (HTTP 404).
  services/.../src/ops/delete_component.rs:179-188
            // The same two failures `GetComponent` tells apart: an id a DIFFERENT
            // app environment owns is a wrong-scope request (the modelled
            // `InvalidParameterException`, 400) ...
            if store.component_in_other_scope(&input.app_id, &input.environment_name, &input.id) {
                return Err(DeleteComponentErrorModeled::InvalidParameterException(
                    crate::state::item_wrong_scope(),
                )
                .into());

The sibling summaries that state the rule the way the code implements it:

  services/.../src/ops/get_form.rs:4-7
    //! ... An id that names no form anywhere is refused with the modelled
    //! `ResourceNotFoundException` (404) and an id a DIFFERENT app environment owns with the
    //! modelled `InvalidParameterException` (400).
  services/.../src/ops/delete_form.rs:6-8
    //! `ResourceNotFoundException` (HTTP 404) with the delete path's own message, and an id
    //! ... `InvalidParameterException` (HTTP 400) — the same two failures `DeleteComponent`

The 400 is pinned by an existing test for exactly these two operations, so the doc (not the code) is the side to correct:

  services/.../tests/all/resource_error_messages.rs:464-506
    /// An id that resolves under ANOTHER app environment answers the wrong-scope refusal ...
    async fn a_wrong_scope_id_answers_one_fixed_phrase() {
        ... ("GetComponent", read(&router, "components", app_id, environment, &id).await),
            ("DeleteComponent", delete_in(&router, app_id, environment, &id).await),
        ... assert_eq!(answer.status, 400, ...)

Also quoted for the rule itself: services/.../src/state.rs:1249-1256 ("The `InvalidParameterException` payload for an id that names a resource another app environment owns: the request is addressed to the wrong scope, which is a different failure from an id that names nothing").

## Steps to Reproduce

1. Read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/get_component.rs lines 4-6: an id that names no component in that app environment is said to answer ResourceNotFoundException (HTTP 404).
2. Read the same file lines 154-172: an id that resolves under another app environment answers InvalidParameterException (HTTP 400) via crate::state::item_wrong_scope(), and only an id that resolves nowhere answers the 404.
3. Read services/.../src/ops/delete_component.rs lines 4-6 and 179-188 for the identical doc/code split.
4. POST /app/d1234abcd/environment/staging/components to create a component, note its id; then GET /app/d9999zzzz/environment/staging/components/{that id}: the answer is HTTP 400 {"message":"The item specified does not belong to that app or environment"}, not the 404 the summary doc promises. DELETE on the same wrong-scope URI answers 400 too.
5. Compare with services/.../src/ops/get_form.rs:4-7 and delete_form.rs:6-8, whose summaries state both branches, and with tests/all/resource_error_messages.rs:464-506, which pins the 400 for GetComponent and DeleteComponent.
