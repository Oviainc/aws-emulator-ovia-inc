Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 055568fd9ce9a7be471ce8dc9e6772f2075ef53d

# A value outside a modelled bound is refused with the unmodelled, TYPED ValidationException instead of this service's InvalidParameterException

_Filed: 2026-10-31T15:45:37Z_
_Operation: CreateComponent | Class: error-response_

> _Note: this issue was identified against the code base at commit `055568fd9ce9a7be471ce8dc9e6772f2075ef53d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service's model declares `InvalidParameterException` (HTTP 400) as a COMMON error on every one of its 28 operations, documented as the refusal of exactly one thing: "An invalid or out-of-range value was supplied for the input parameter." Every refusal of that class the crate's own handlers own is answered with that shape (PutMetadataFlag for a feature name outside the catalogue, ListComponents/ExportComponents/ExportForms/ExportThemes for a nextToken the operation never issued, Get/Delete/UpdateComponent and the form paths for a wrong-scope id).

A value outside a bound the MODEL declares is not. `ComponentName` is `@length(min: 1, max: 255)`, so `componentToCreate.name` with 300 characters is refused by the framework's validating walk before the handler runs, and this crate leaves that refusal exactly as the framework rendered it: `ValidationException`, a shape this service's model does not declare, carrying the `x-amzn-errortype` header and a `__type` body member. That is the one refusal class of this service that both (a) names a shape the model does not declare and (b) names an error type on the wire at all — the crate's own response path states that AmplifyUIBuilder "answers a refused call with the status and a `message`, and with NO error-type discriminator" and then deliberately exempts `ValidationException` from the strip on the grounds that "AmplifyUIBuilder has no such errors", which is true of `ValidationException` and is precisely why the model's own `InvalidParameterException` is what an out-of-range input value should be reported as.

The crate already owns the seam for this and leaves it unused: `EmulatedService::override_refusal` is documented as the hook the centralized walk calls "on a framework-level refusal (an absent required member, or a value outside a declared bound) to let this service reword it" and returns `None`. So two requests that fail for the same documented reason — an invalid input value — get two different error shapes, one untyped and one typed, depending on whether the bound that caught it was modelled or hand-checked.

The same divergence is reachable on every operation with a modelled constraint (e.g. `ListComponents?maxResults=0` against `ListEntityLimit` `@range(min: 1, max: 100)`, `StartCodegenJob` with an `appId` outside `^d[a-z0-9]+$`); CreateComponent is named here as the first and most common one.

## Evidence

The model, for every operation of this service (service-level common error; `smithy_query(query="digest")` reports `"common_errors": ["InvalidParameterException"]`):

  smithy_query(query="shape", shape="InvalidParameterException")
    "errorKind": "client", "httpError": 400,
    "doc": "<p>An invalid or out-of-range value was supplied for the input parameter.</p>"
  smithy_query(query="shape", shape="ComponentName")
    "type": "string", "length": {"min": 1, "max": 255}

The seam this crate owns for a framework-level bound refusal, left unused:

  services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/service.rs:118-129
    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. ...
        let _ = refusal;
        None
    }

The crate's own statement of this service's error wire form, and the ValidationException carve-out:

  services/.../src/service/untyped_errors.rs:3-7
    //! How this service names the exceptions it answers on the wire: it does not.
    //!
    //! AmplifyUIBuilder answers a refused call with the status and a `message`, and with NO
    //! error-type discriminator — neither the `x-amzn-errortype` header restJson1 resolves an
    //! error shape from, nor a `__type` / `code` / `Code` member in the body
  services/.../src/service/untyped_errors.rs:28-33
    //! What it deliberately leaves typed is a code that is NOT one of this service's modelled
    //! exceptions (see [`MODELLED_EXCEPTIONS`]): the scaffold's `NotImplemented` (501) and the
    //! framework's `ValidationException` (400) ... AmplifyUIBuilder has no such errors
  services/.../src/service/untyped_errors.rs:303-309 (the unit test pinning that a framework refusal keeps its type)
    for (label, body) in [
        ("the scaffold's 501", r#"{"__type":"NotImplemented","Message":"no"}"#),
        (
            "a framework refusal",
            r#"{"__type":"ValidationException","Message":"no"}"#,
        ),

The same class of failure, answered by the crate's own handlers with the MODELLED shape:

  services/.../src/ops/put_metadata_flag.rs:200-211
    // `featureName` is an `@httpLabel` targeting a plain `String`, so the
    // model cannot express which names exist: ... a name outside it is an invalid parameter
    // rather than a new feature to store.
    if !crate::state::is_known_metadata_feature(&input.feature_name) {
        return Err(PutMetadataFlagErrorModeled::InvalidParameterException(
  services/.../src/ops/list_components.rs:186-193
    return Err(ListComponentsErrorModeled::InvalidParameterException(
        InvalidParameterException { message: Some(format!(
            "nextToken {token:?} is not a token returned by ListComponents for this app environment"

Test coverage today: the crate's rule is asserted over handler-produced refusals only, and its own wording is broader than what the code delivers —

  services/.../tests/all/component_refusals.rs:474-476
    /// Rule 3: across every refusal this service answers — each status class, each operation —
    /// the response names no error type; the scaffold's 501 for an unwritten operation still
    /// does, because it reports something no AmplifyUIBuilder exception reports.
  services/.../tests/all/component_refusals.rs:484-537 — the rows are a duplicate name (409), a reserved property name (400), absent ids (404), a wrong-scope id (400) and an unknown metadata feature (400); NO row sends a value outside a modelled `@length`/`@range`/`@pattern`, so the one refusal class that does name an error type is untested. Routing the framework refusal onto `InvalidParameterException` through `override_refusal` leaves every assertion in that file, in `resource_error_messages.rs` and in the `untyped_errors.rs` unit tests (which exercise `without_discriminator` directly) unchanged.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/components with componentToCreate = {"name": "<301 'a' characters>", "componentType": "View", "properties": {}, "variants": [], "overrides": {}, "bindingProperties": {}} — `ComponentName` is @length(1,255), so the validating walk refuses it before create_component.rs runs.
2. Observe the response: HTTP 400 carrying `x-amzn-errortype: ValidationException` and a body whose `__type` is `ValidationException` — a shape `com.amazonaws.amplifyuibuilder` does not declare.
3. Now POST the same URI with componentToCreate = {"name": "Reserved", "componentType": "View", "properties": {"tags": {"value": "x"}}, "variants": [], "overrides": {}, "bindingProperties": {}} — the handler's own invalid-parameter refusal.
4. Observe: HTTP 400 with NO `x-amzn-errortype` header and no `__type` member, i.e. the modelled `InvalidParameterException` as this service answers it (src/service/untyped_errors.rs).
5. Same divergence: GET /app/d1234abcd/environment/staging/components?maxResults=0 (against `ListEntityLimit` @range(1,100)) answers the typed ValidationException, while GET ...?nextToken=bogus answers the untyped InvalidParameterException from list_components.rs:186.
