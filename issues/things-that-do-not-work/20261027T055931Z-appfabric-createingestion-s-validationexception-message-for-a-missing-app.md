Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.create-ingestion-without-app-authorization-rejected
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|CreateIngestion|ValidationException
Issue-Commit: 12b77c8390fc415c090881564953782802c727cf

# CreateIngestion's ValidationException message for a missing app authorization is emulator-authored prose, not the message AWS returns

> _Note: this issue was identified against the code base at commit `12b77c8390fc415c090881564953782802c727cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIngestion

## Description
One cause.

Cause 1 of 1: the message of the modelled ValidationException that CreateIngestion returns when the named app bundle holds NO app authorization at all for the request's (app, tenantId) pair is rewritten by the emulator, and interpolates the requested app name into text AWS states as fixed prose.

AWS: Smithy (appfabric, FabricFrontendService) models CreateIngestion POST /appbundles/{appBundleIdentifier}/ingestions -> 201, with error ValidationException (errorKind client, httpError 400, members message/reason/fieldList, doc "The request has invalid or missing parameters."). The model does not state the message text, so for the text this is recorded behavior only: AppFabric answers an ingestion request for an (app, tenant) pair with no app authorization with HTTP 400, code ValidationException, and the message "The app authorization for the specified application and tenant was not found. Create an app authorization for the application and tenant before creating an ingestion." Every word of that message is FIXED TEXT — it has no variable part at all: AWS deliberately says "the specified application and tenant" rather than echoing the app or tenant from the request. Nothing in it comes from the request or from stored state.

Emulator: returns the right status (400) and the right code (ValidationException), but the message is "No app authorization exists for app SLACK and the specified tenant; create one and connect it before creating an ingestion" — different wording, different sentence structure, no trailing period, and with the request's `app` value spliced in where AWS has fixed text ("SLACK" here is just the value the replayed request carried; the defect is the interpolation, not that one value). The text comes from `crate::state::app_authorization_not_connected(app, None)`, whose `None` arm formats "No app authorization exists for app {app} and the specified tenant; create one and connect it before creating an ingestion"; the handler calls it when `connected_authorization(app, tenant_id)` is None. The sibling arm (`Some(status)`, authorization present but not Connected) is the same hand-authored family and should be re-derived against AWS at the same time.

Why: clients and tooling match on this message. AppFabric reports several distinct precondition failures through one code (ValidationException) and one status (400), so the message is the only thing that distinguishes "you never created the authorization" from "you created it but did not connect it" and from field-level validation failures. A client or test that keys on AWS's sentence — or on the fact that the sentence contains no request-derived value — classifies the emulator's refusal as an unknown error, logs the wrong remediation, or fails a fixture comparison. Interpolating the app name additionally leaks request content into a message AWS keeps value-free, so golden-response diffs never stabilise.

Fix: the error-message constructor in the service's state module, `app_authorization_not_connected` in services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs (the `None` arm), not the handler's control flow — the precondition is detected in the right place and classified with the right error shape and status; only the rendered message diverges. The handler call site in src/ops/create_ingestion.rs passes the held status, so the fix can keep that signature and just emit AWS's fixed sentence for the no-authorization case.

Note: this is NOT the same defect as the open issue 20261027T015653Z-appfabric-createingestion-wrong-behavior--r2.md ("CreateIngestion evaluates the app-authorization precondition before the idempotency replay"). That issue is about the ORDER of the precondition relative to the clientToken replay; this one is about the TEXT of the refusal and applies even when the ordering is correct.

Traces: appfabric.create-ingestion-without-app-authorization-rejected (verdict mismatch) — the check creates a fresh app bundle, calls CreateIngestion with app=SLACK, a fresh tenantId and ingestionType=auditLog without ever creating an app authorization, asserts code == ValidationException (passes) and then asserts the exact AWS message (fails).

## Evidence
Failing check: appfabric.create-ingestion-without-app-authorization-rejected [mismatch]

AssertionError: expected the exact ValidationException message AWS returns, got 'No app authorization exists for app SLACK and the specified tenant; create one and connect it before creating an ingestion'

Expected (AWS, HTTP 400, Error.Code = ValidationException, all fixed text, no variable parts):
  "The app authorization for the specified application and tenant was not found. Create an app authorization for the application and tenant before creating an ingestion."

Actual (emulator, HTTP 400, Error.Code = ValidationException):
  "No app authorization exists for app <app> and the specified tenant; create one and connect it before creating an ingestion"
  — where <app> is the request's `app` member (the recorded run shows SLACK); AWS has fixed text in that position.

Status and error code agree; only the message diverges.

Source of the divergent text — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs:1653-1664:
```
pub fn app_authorization_not_connected(app: &str, status: Option<&str>) -> ValidationException {
    precondition_not_met(match status {
        Some(status) => format!(
            "The app authorization for app {app} and the specified tenant is {status}, \
             not {APP_AUTHORIZATION_STATUS_CONNECTED}"
        ),
        None => format!(
            "No app authorization exists for app {app} and the specified tenant; \
             create one and connect it before creating an ingestion"
        ),
    })
}
```

Call site — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_ingestion.rs:250-262:
```
let parent = store.app_bundles.get(&bundle_key);
if parent.and_then(|bundle| bundle.connected_authorization(&input.app, &input.tenant_id)).is_none() {
    let held_status = parent.and_then(|b| b.authorization_for(&input.app, &input.tenant_id)).map(|a| a.status.clone());
    return Err(CreateIngestionErrorModeled::ValidationException(
        crate::state::app_authorization_not_connected(&input.app, held_status.as_deref()),
    ).into());
}
```

Model (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json): CreateIngestion errors include ValidationException {errorKind: client, httpError: 400}; ValidationException members = message (required), reason (required enum), fieldList. The model does not fix the message text — the expected sentence above is recorded AWS behavior.

## Steps to Reproduce
1. Call CreateAppBundle (region us-east-1) with a fresh clientToken -> 201; note appBundle.arn (bundle B). Delete any pre-existing bundle first: AppFabric allows one bundle per account/region.
2. Do NOT create any app authorization under B.
3. POST /appbundles/{B}/ingestions with {"app":"SLACK","tenantId":"<fresh tenant id>","ingestionType":"auditLog"}.
4. Observe HTTP 400 with Error.Code = ValidationException (correct) and Error.Message = "No app authorization exists for app SLACK and the specified tenant; create one and connect it before creating an ingestion".
5. AWS returns, for this same request, Error.Message = "The app authorization for the specified application and tenant was not found. Create an app authorization for the application and tenant before creating an ingestion." — fixed text with no request-derived values.
6. ListIngestions on B returns [] in both cases, so only the message diverges.
