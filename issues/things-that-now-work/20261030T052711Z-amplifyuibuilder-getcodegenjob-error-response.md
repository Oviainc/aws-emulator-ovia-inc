Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4f810a43fdffae0cccf3176a3d509af53000869e

# GetCodegenJob reports an id another app environment owns as absent (404) instead of the crate's wrong-scope 400

_Filed: 2026-10-30T05:27:11Z_
_Operation: GetCodegenJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `4f810a43fdffae0cccf3176a3d509af53000869e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate implements ONE rule for a resource addressed by (appId, environmentName, id): an id that resolves under a DIFFERENT app environment is a WRONG-SCOPE request, refused with the modelled `InvalidParameterException` (400, message "The item specified does not belong to that app or environment"), and is told apart from an id that names no resource anywhere, which is absent (`ResourceNotFoundException`, 404). The crate states that rule in `state.rs` (`item_wrong_scope()` / `WRONG_SCOPE_MESSAGE`) and its own test suite states it is "per CLASS, not per operation or resource kind".

The codegen job is addressed exactly that way — `CodegenJobResource` identifiers are `{id: Uuid, appId: AppId, environmentName: String}`, the same triple as `FormResource` and `ComponentResource` — yet `GetCodegenJob` consults only the addressed app environment and reports EVERY miss as absent. So the identical mis-addressing ("this job's id, asked for under another app or another environment of the same app") is answered 400 for a component or a form and 404 for a codegen job.

Note this is NOT the already-filed theme gap (20261030T033747Z-amplifyuibuilder-gettheme-error-response): that issue enumerates "Six of the nine handlers addressed that way" — the three component and three form handlers versus the three theme handlers — and never mentions the codegen job, and `delete_theme.rs`'s admission names only components and forms as the kinds that classify it differently. The codegen job is a fourth kind addressed by the same triple and left off the rule with no comment at all.

Effect: a client that branches on 400-vs-404 to tell "I asked in the wrong scope" from "this job is gone" gets a different answer depending on which of the service's resources it addressed.

## Evidence

The rule, as this crate's state module states it (generically, over "an id that resolves"):

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1186-1193
    /// The message of the wrong-scope refusal — an id that resolves, but under
    /// another app or backend environment than the one the request addressed.
    ...
    pub const WRONG_SCOPE_MESSAGE: &str =
        "The item specified does not belong to that app or environment";
  services/.../src/state.rs:1213-1216
    /// The `InvalidParameterException` payload for an id that names a resource
    /// another app environment owns: the request is addressed to the wrong scope,
    /// which is a different failure from an id that names nothing.
    pub fn item_wrong_scope() -> InvalidParameterException {

Sibling READ handlers of this crate that APPLY it:

  services/.../src/ops/get_component.rs:162-167
    if store.component_in_other_scope(&input.app_id, &input.environment_name, &input.id) {
        return Err(GetComponentErrorModeled::InvalidParameterException(
            crate::state::item_wrong_scope(),
        )
        .into());
    }
  services/.../src/ops/get_form.rs:161-166 (same, via `form_in_other_scope`)

The handler under review, which consults nothing outside the addressed scope — every miss is the read-path not-found literal:

  services/.../src/ops/get_codegen_job.rs:163-168
    let Some(job) = store.codegen_job(&input.app_id, &input.environment_name, &input.id) else {
        return Err(GetCodegenJobErrorModeled::ResourceNotFoundException(
            crate::state::item_not_found(),
        )
        .into());
    };

That the model addresses a codegen job exactly as it addresses a form:

  smithy_query(query="shape", shape="CodegenJobResource") →
    "identifiers": {"id": "Uuid", "appId": "AppId", "environmentName": "String"},
    "arn": {"template": "app/{appId}/environment/{environmentName}/codegen-jobs/{id}"}
  smithy_query(query="shape", shape="FormResource") →
    "identifiers": {"id": "Uuid", "appId": "String", "environmentName": "String"}

`GetCodegenJob` models `InvalidParameterException` (400) and this handler can never construct it:
  smithy_query(query="operation", operation="GetCodegenJob") → errors include
    {"shape": "InvalidParameterException", "errorKind": "client", "httpError": 400}

The crate's own test suite states the rule is per class, not per resource kind:

  services/.../tests/all/resource_error_messages.rs:11-19
    //! literal that interpolates nothing from the request ... and the literal is per CLASS, not per operation
    //! or resource kind:
    //! | an id another app environment owns        | 400    | contains "does not belong to that app or environment"|

Test coverage today: the 400 is pinned for components and forms (tests/all/component_refusals.rs, tests/all/form_delete.rs, tests/all/form_update.rs — rows asserting the "does not belong to that app or environment" phrase). The 404 is pinned for codegen jobs with NO reason stated for the cross-scope rows — the module doc's rule 4 only says the miss must not be an empty success:

  services/.../tests/all/codegen_job_lifecycle.rs:18-19
    //! 4. An id naming no job in the app environment addressed is the modelled
    //!    `ResourceNotFoundException` (HTTP 404), not an empty success.
  services/.../tests/all/codegen_job_lifecycle.rs:712-723
            ("a real id read from another environment", APP_ID.to_string(), "production".to_string(), id.clone()),
            ("a real id read from another app", "d9999zzzz".to_string(), ENVIRONMENT.to_string(), id.clone()),
  …assert_eq!(answer.status, 404, …) at :728-732 — those two rows move to 400 with crate::state::WRONG_SCOPE_MESSAGE when the codegen side of the rule lands; the other two rows ("a UUID nothing ever started", "an id that is not a UUID at all") stay 404.

`grep in_other_scope src/` matches `component_*` and `form_*` only — there is no `codegen_job_in_other_scope` helper.

## Steps to Reproduce

1. Mount the service on one engine (as tests/all/codegen_job_lifecycle.rs does).
2. POST /app/d1234abcd/environment/staging/codegen-jobs with codegenJobToCreate = {"renderConfig": {"react": {}}} — note the returned job id J.
3. POST /app/d1234abcd/environment/staging/components with a valid componentToCreate — note the returned id C.
4. GET /app/d9999zzzz/environment/staging/components/C (the component's id asked for under another app): answered HTTP 400 with {"message":"The item specified does not belong to that app or environment"} (get_component.rs:162-167).
5. GET /app/d9999zzzz/environment/staging/codegen-jobs/J (the job's id asked for under another app, the identical mis-addressing): answered HTTP 404 with {"message":"The item specified was not found"} instead of the wrong-scope 400 (get_codegen_job.rs:163-168).
6. Same divergence for the other half of the key: GET /app/d1234abcd/environment/production/codegen-jobs/J → 404, where /app/d1234abcd/environment/production/components/C → 400.
