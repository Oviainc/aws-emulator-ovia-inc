Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 889b7a0ea20193cdb3c9577846ef2b90647eb8e8

# A boundary-caught constraint violation answers a ValidationException body with no `reason`, the member the model marks @required and every handler-built refusal of the same shape fills

_Filed: 2026-10-29T08:01:25Z_
_Operation: CreateEnvironment | Class: error-response_

> _Note: this issue was identified against the code base at commit `889b7a0ea20193cdb3c9577846ef2b90647eb8e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon EVS models exactly one client-input error, `com.amazonaws.evs#ValidationException`, and that shape declares `reason` as @required (`ValidationExceptionReason`, one of `unknownOperation | cannotParse | fieldValidationFailed | other`). This crate honours that on every path it builds the shape itself: `aggregate_validation_error` fills `reason` with `fieldValidationFailed` and `request_validation_error` with `other`, and the generated type makes the member non-optional (`pub reason: String`).

The one path that answers the same shape WITHOUT it is the boundary path. A request refused by the framework's own validator — an absent @required member, a value outside a declared @length/@range, a @pattern or enum violation — never reaches a handler; it is worded by `EmulatedService::override_refusal`, whose return type carries only `code` and `message`, so the body the caller receives is exactly `{"__type":"ValidationException","message":"…"}`. `reason` is absent, and so is `fieldList`.

CreateEnvironment makes the divergence exact on ONE member: `vpcId` refused for its @pattern (`vpc-zzzzzzzz`) is caught at the boundary and answers a body with no `reason`, while `vpcId` refused because the account's EC2 store holds no such VPC is caught in the handler and answers `reason: "fieldValidationFailed"` plus a `fieldList` entry named `vpcId`. Two refusals of the same member of the same operation produce two differently shaped `ValidationException` bodies, and the model's own `ValidationExceptionReason` set even provides the values the boundary cases need (`fieldValidationFailed` for a failed field, `cannotParse` for a body that cannot be read). The crate already treats the @required members of an error shape as part of the contract — `error_wire_status.rs` asserts all three required members of `ResourceNotFoundException` (`message`, `resourceId`, `resourceType`) on every not-found body — so dropping a required member on the refusal path every one of the 24 operations shares is a defect rather than a choice. A client that builds the modelled shape from the body (a required member with no value) cannot deserialize the refusal at all, so the 400 it reports is not merely under-populated but unreadable as the modelled error.

## Evidence

The model requires the member — `smithy_query(query="shape", shape="ValidationException")`:
```
"ValidationException": {"type": "structure", "errorKind": "client", "httpError": 400, "members": {
  "message": {"target": "String", "required": true, …},
  "reason": {"target": "ValidationExceptionReason", "required": true, "type": "enum", "doc": "<p>The reason for the exception.</p>"},
  "fieldList": …}}
```
and `smithy_query(query="shape", shape="ValidationExceptionReason")` -> `["unknownOperation", "cannotParse", "fieldValidationFailed", "other"]`.

The generated type makes it non-optional — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/types/shared_types.rs:34-40:
```
#[smithy_macros::smithy(shape = "com.amazonaws.evs#ValidationException")]
pub struct ValidationException {
    pub message: String,
    pub reason: String,
    pub field_list: Option<Vec<ValidationExceptionField>>,
}
```

The handler path always fills it — src/state.rs:154-157 and src/state.rs:404-408, 514-519:
```
/// The `ValidationExceptionReason` values this service refuses with.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";
/// See [`REASON_FIELD_VALIDATION_FAILED`].
pub const REASON_OTHER: &str = "other";
```
```
    ValidationException {
        message: format!("{VALIDATION_FAILURE_PREFIX}{joined}"),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(failures),
    }
```
```
pub fn request_validation_error(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        reason: REASON_OTHER.to_string(),
        field_list: None,
    }
```

The boundary path cannot carry it — src/service.rs:115-122:
```
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        Some(Refused {
            code: "ValidationException".to_string(),
            message: crate::state::constraint_violation_message(violations)?,
        })
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
so the awsJson body is only those two fields — vendor/smithy-server-runtime/src/pre_dispatch.rs:438-440:
```
    let (type_name, message) = apply(chosen, type_name, message);
    let message = escape_json(&message);
    let body = format!(r#"{{"__type":"{type_name}","message":"{message}"}}"#);
```

The crate's own tests pin `reason` on the handler path — tests/all/create_environment_validation.rs:35 and 82-87:
```
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";
...
    assert_eq!(
        body["reason"],
        json!(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {}",
        body["reason"]
    );
```
(the same assertion in tests/all/environment_deletion_hosts.rs:168-171), and the crate asserts the full required-member set of the other error shape — tests/all/error_wire_status.rs:105-119 (`resourceId` must echo the requested id, `resourceType` must name the resource).

Nothing pins the ABSENCE on the boundary path, so filling `reason` leaves every existing expectation unchanged: tests/all/validation_message_rendering.rs reads only `__type` and `message` off the body (lines 94-103) and asserts only the status, the code and the exact message (`assert_refused`, lines 218-246).

## Steps to Reproduce

1. Boot the emulator and point an evs client at it (awsJson1_0, POST / with X-Amz-Target: AmazonElasticVMwareService.<Op>).
2. Call CreateEnvironment with vpcId="vpc-zzzzzzzz" (a @pattern violation caught at the boundary) and otherwise valid members — observe HTTP 400 with body {"__type":"ValidationException","message":"1 validation error detected: Value at 'vpcId' failed to satisfy constraint: …"} and NO "reason" member.
3. Call CreateEnvironment omitting vpcId entirely (an absent @required member, also caught at the boundary) — again a ValidationException body with no "reason".
4. Now call CreateEnvironment with a well-formed vpcId the account's EC2 store does not hold (the same member, refused in the handler) — observe HTTP 400 with "reason":"fieldValidationFailed" and a "fieldList" entry named "vpcId", the shape tests/all/create_environment_validation.rs:82-87 asserts.
5. Repeat the contrast on another operation to see it is service-wide: ListEnvironments with maxResults=101 (boundary, @range) answers without "reason", while ListEnvironments with a nextToken this service never issued (handler, crate::state::invalid_next_token) answers with "reason":"fieldValidationFailed".
