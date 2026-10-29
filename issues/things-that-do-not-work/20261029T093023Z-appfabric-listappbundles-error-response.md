Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# A boundary-caught ValidationException answers without `reason`, the member the model marks @required and every handler-built refusal of the same shape fills

_Filed: 2026-10-29T09:30:23Z_
_Operation: ListAppBundles | Class: error-response_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AppFabric models `ValidationException` with `reason` as @required, and this crate honours that on every path where a handler builds the shape: `crate::state::precondition_not_met` fills `reason: "other"` and `crate::state::field_validation_failed` fills `reason: "fieldValidationFailed"` plus a `fieldList`. The generated type even makes the member non-optional (`pub reason: String`).

The one path that answers the SAME shape without it is the boundary path. A request the framework's own validator refuses — a value outside a declared `@range`/`@length`, a `@pattern` violation, an absent @required member — never reaches a handler; it is worded by `EmulatedService::override_refusal`, whose return type carries only `code` and `message`, so the rendered body is exactly `{"__type":"ValidationException","message":"…"}` with no `reason` (and no `fieldList`).

`ListAppBundles` makes the divergence exact inside ONE operation: `?maxResults=101` is refused at the boundary and answers a body with no `reason`, while `?nextToken=<something this service never issued>` is refused in the handler (through `decode_page_token` -> `field_validation_failed`) and answers `reason: "fieldValidationFailed"` with a `fieldList` entry named `nextToken`. Two refusals of the same operation produce two differently shaped `ValidationException` bodies, and a client that deserializes the modelled shape (a required member with no value) cannot read the boundary one as the modelled error at all. The model's own `ValidationExceptionReason` set supplies the values the boundary cases need (`fieldValidationFailed` for a failed field, `cannotParse` for a body that cannot be read). The same seam is shared by every one of the 26 operations, so the gap is service-wide; `ListAppBundles` is the first operation whose boundary refusal the crate's own tests exercise.

## Evidence

The model requires the member — `smithy_query(query="shape", shape="ValidationException")`:
```
"ValidationException": {"type": "structure", "errorKind": "client", "httpError": 400, "members": {
  "message": {"target": "String", "required": true},
  "reason": {"target": "ValidationExceptionReason", "required": true, "type": "enum", "doc": "<p>The reason for the exception.</p>"},
  "fieldList": {"target": "ValidationExceptionFieldList", "type": "list"}}}
```
and `smithy_query(query="shape", shape="ValidationExceptionReason")` -> `["unknownOperation", "cannotParse", "fieldValidationFailed", "other"]`.

The generated type makes it non-optional — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/types/shared_types.rs:51-57:
```
#[smithy_macros::smithy(shape = "com.amazonaws.appfabric#ValidationException")]
#[derive(Debug, Clone, Default)]
pub struct ValidationException {
    pub message: String,
    pub reason: String,
    pub field_list: Option<Vec<ValidationExceptionField>>,
}
```

Every handler path fills it — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs:1561-1578:
```
pub fn precondition_not_met(message: String) -> ValidationException {
    ValidationException {
        message,
        reason: "other".to_string(),
        field_list: None,
    }
}
...
pub fn field_validation_failed(member: &str, message: &str) -> ValidationException {
    ValidationException {
        message: message.to_string(),
        reason: "fieldValidationFailed".to_string(),
        field_list: Some(vec![ValidationExceptionField { ... }]),
    }
}
```

The boundary path cannot carry it — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/service.rs:172-178:
```
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
```
and `Refused` has no third member — vendor/smithy-server-runtime/src/pre_dispatch.rs:163-169:
```
pub struct Refused {
    /// Error code / `__type` shape name.
    pub code: String,
    /// Human-readable message, unescaped — each protocol escapes it itself.
    pub message: String,
}
```
so the rendered body is only those two fields — vendor/smithy-server-runtime/src/pre_dispatch.rs:440:
```
    let body = format!(r#"{{"__type":"{type_name}","message":"{message}"}}"#);
```

Nothing pins the ABSENCE, so filling `reason` leaves every existing expectation unchanged. The boundary test asserts only status, code and message — tests/all/app_authorization_and_ingestion_dependency.rs:1165-1189 (`assert_eq!(answer.status, 400 …)`, `assert_eq!(answer.error_code(), "ValidationException" …)`, `assert_eq!(message(&answer), format!("1 validation error detected: Value at 'maxResults' failed to satisfy constraint: {requirement}"))`), and the handler-path token test asserts only status and code — tests/all/app_bundle_ingestion_lifecycle.rs:654-664.

## Steps to Reproduce

1. Boot the emulator and point an appfabric client (restJson1) at it.
2. GET /appbundles?maxResults=101 — observe HTTP 400 with body {"__type":"ValidationException","message":"1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"} and NO "reason" member, although the model marks ValidationException$reason @required.
3. GET /appbundles?nextToken=not-a-token — the same operation, refused in the handler, answers HTTP 400 with "reason":"fieldValidationFailed" and a "fieldList" entry named "nextToken".
4. Repeat on any other operation to see it is service-wide, e.g. GET /appbundles/{id}/ingestions?maxResults=0 (boundary, no reason) versus the same listing with an unissued nextToken (handler, reason present).
5. Deserialize either body into the modelled ValidationException shape: the boundary one fails because the required `reason` member has no value.
