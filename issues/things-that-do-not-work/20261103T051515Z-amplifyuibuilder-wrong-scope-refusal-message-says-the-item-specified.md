Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifyuibuilder.get-component-cross-environment-isolation-rejected
Issue-Shape-Id: com.amazonaws.amplifyuibuilder#AmplifyUIBuilder
Issue-Key: amplifyuibuilder|GetComponent|400
Issue-Commit: 6f0c16a0b241f53b494cf0d5c14f6ff644768faf

# Wrong-scope refusal message says "The item specified does not belong..." where AWS says "The item requests does not belong..."

> _Note: this issue was identified against the code base at commit `6f0c16a0b241f53b494cf0d5c14f6ff644768faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComponent

## Description
Cause 1 of 1: the fixed text of the wrong-scope (cross-app/cross-environment isolation) refusal is wrong by one word, so every operation that refuses an id another app environment owns serves a message no AWS client can match. Failing check: amplifyuibuilder.get-component-cross-environment-isolation-rejected.

AWS: GetComponent is modelled with errors InternalServerException (500), InvalidParameterException (400) and ResourceNotFoundException (404) (Smithy: GetComponent errors; http GET /app/{appId}/environment/{environmentName}/components/{id}). The model does not state the message text, so for the text this is recorded behavior only: fetching a component id under a DIFFERENT appId (same environmentName) is refused with HTTP 400, Error.Code "400" (no __type in the body), and Error.Message exactly "The item requests does not belong to that app or environment". That whole string is FIXED text — the message interpolates no value from the request or from state (no appId, environmentName or id appears in it), so there is nothing per-run in it and nothing to template.

Emulator: status 400 and Code "400" are right, but the message is "The item specified does not belong to that app or environment" — "specified" where AWS has "requests". The string is a single crate-wide constant, so the divergence is served by every path that uses it, not only GetComponent: services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1329-1330 defines WRONG_SCOPE_MESSAGE and state.rs:1353-1357 (item_wrong_scope()) wraps it in the InvalidParameterException returned by get_component.rs, update_component.rs, delete_component.rs, get_form.rs, update_form.rs and delete_form.rs.

Why: the wrong-scope 400 is untyped (Code "400", no __type), so the message's fixed phrase is the ONLY thing that separates a scope violation from any other malformed-parameter 400 — the crate's own doc comment at state.rs:1323-1328 says exactly that. A client (or an AWS-recorded test) that matches on the AWS phrase sees no match and must treat the refusal as an unclassified 400, losing the ability to tell "you asked in the wrong app/environment" from "your parameter was bad". Error-message assertions that pass against AWS fail against the emulator.

Fix: the message constant in the crate's shared error payloads — WRONG_SCOPE_MESSAGE in src/state.rs, consumed by item_wrong_scope(); no handler logic, status code or error classification changes. Note the crate's doc comment also quotes the phrase, and tests/all/resource_error_messages.rs and tests/all/component_refusals.rs / form_delete.rs / form_update.rs pin the current wording, so they move with it. This is NOT the same defect as the already-open issues 20261030T184530Z-amplifyuibuilder-getcomponent-error-response (stale doc summaries promising 404 where the handler answers this 400) or 20261030T033747Z-amplifyuibuilder-gettheme-error-response (theme handlers answering 404 instead of this 400); both of those take the current message text as correct, and both quote the wrong string.

Traces: amplifyuibuilder.get-component-cross-environment-isolation-rejected — create two apps, create a component under app A, GetComponent that id under app B with the same environmentName; the positive control (the same id under app A) succeeds in the emulator, so only the refusal message diverges.

## Evidence
Failing check amplifyuibuilder.get-component-cross-environment-isolation-rejected:

  AssertionError: expected the cross-app-isolation message, got
  {'Message': 'The item specified does not belong to that app or environment', 'Code': '400'}

Expected (AWS, from the recording the check asserts):
  HTTP 400, Error.Code "400" (no __type)
  Error.Message == "The item requests does not belong to that app or environment"

Actual (emulator):
  HTTP 400, Error.Code "400"
  Error.Message == "The item specified does not belong to that app or environment"

Only the fixed message text diverges: the status assertion (400) and the positive control (GetComponent under the owning app returns the component with the same id and name) both passed.

Source of the diverging string:
  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1323-1330
    /// The message of the wrong-scope refusal - an id that resolves, but under
    /// another app or backend environment than the one the request addressed.
    ///
    /// Recorded behaviour: the message contains the fixed phrase "does not belong to
    /// that app or environment", which names no ids; ...
    pub const WRONG_SCOPE_MESSAGE: &str =
        "The item specified does not belong to that app or environment";

  services/.../src/state.rs:1350-1357
    pub fn item_wrong_scope() -> InvalidParameterException {
        InvalidParameterException {
            message: Some(WRONG_SCOPE_MESSAGE.to_string()),
        }
    }

All callers serve the same text: src/ops/get_component.rs (~:162-167), update_component.rs (~:261), delete_component.rs (~:183), get_form.rs (~:161), update_form.rs (~:181), delete_form.rs (~:184).

Model context (no message text is modelled, so the text is recorded-only):
  smithy_query(operation="GetComponent") -> errors: InternalServerException(500), InvalidParameterException(400), ResourceNotFoundException(404)

## Steps to Reproduce
1. Create two Amplify apps (amplify CreateApp) -> appId A and appId B.
2. amplifyuibuilder CreateComponent under app A, environmentName "staging", with a minimal componentToCreate (name, componentType "Text", properties {label: {value: "scoped"}}, empty variants/overrides/bindingProperties). Note the returned entity id C.
3. Call GetComponent(appId=B, environmentName="staging", id=C) — i.e. GET /app/B/environment/staging/components/C.
4. Observe the refusal: HTTP 400, body {"message":"The item specified does not belong to that app or environment"} (Error.Code "400", no __type). AWS answers the same status and code with the message "The item requests does not belong to that app or environment".
5. Positive control: GetComponent(appId=A, environmentName="staging", id=C) still returns the component, so only the refusal's message text diverges.
6. The same wrong text is served by UpdateComponent, DeleteComponent, GetForm, UpdateForm and DeleteForm for a wrong-scope id, since all read services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1329 WRONG_SCOPE_MESSAGE.
