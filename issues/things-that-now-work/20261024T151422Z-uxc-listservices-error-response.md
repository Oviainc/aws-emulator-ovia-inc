Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# ValidationException for maxResults/nextToken member violations omits the modelled fieldList

_Filed: 2026-10-24T15:14:22Z_
_Operation: ListServices | Class: error-response_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListServices binds both inputs with @httpQuery, so a `maxResults` outside @range(1,1000) or a `nextToken` outside @length(32,512)/@pattern is refused with the modelled ValidationException (HTTP 400). The emulator renders that refusal with only a `message` member: the body is `{"message": "1 validation error(s) detected: Value '0' at 'maxResults' failed to satisfy constraint: ..."}` and no `fieldList`.

Expected: a member-constraint refusal identifies the offending member structurally as well as in prose — ValidationException.fieldList carrying one ValidationExceptionField per violated member (`path` = the member path, e.g. `maxResults`/`nextToken`, `message` = the constraint text), which is exactly what the model defines that member for ("The list of fields that are invalid"). A client that inspects fieldList to report which parameter was wrong sees nothing to inspect against the emulator.

Actual: the service's refusal seam (`EmulatedService::override_refusal` in src/service.rs) can only return `Refused { code, message }`, so no fieldList is ever serialized for any refusal of this service. The handler even contains the correct shape — `invalid_member()` in src/ops/list_services.rs builds a ValidationException with `field_list: Some(vec![ValidationExceptionField { path, message }])` and its doc comment says it carries the member path "the way the service's own boundary refusal does" — but that helper is unreachable: the framework's boundary validation refuses an out-of-range `maxResults` before `handle` runs, so the only refusal that reaches the wire is the fieldList-less one from `override_refusal`.

## Evidence

Spec: smithy_query(query="shape", shape="ValidationException") — members `message` (required) and `fieldList` (ValidationExceptionFieldList), documented "The list of fields that are invalid"; ValidationExceptionField has required `path` ("The field name with the validation exception") and `message`. ListServices declares ValidationException (HTTP 400) and binds `maxResults` (@range 1..1000) and `nextToken` (@length 32..512, @pattern) with @httpQuery, per smithy_query(query="operation", operation="ListServices").

Handler: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/service.rs lines 342-380 — `override_refusal` returns `Some(Refused { code: "ValidationException", message: envelope(&restated)? })`, i.e. code + message only; `Refused` has no fieldList channel, so the serialized error body has no `fieldList` for any URI-bound member violation. services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/list_services.rs lines 155-166 define `invalid_member()` with a populated `field_list`, but the only caller (lines 246-257, the `maxResults` range guard inside `handle`) is downstream of the framework's own validation, so it never fires for a request that violates the modelled range.

Tests: tests/all/account_customizations.rs::a_member_constraint_violation_is_not_reported_as_a_pagination_token_exception (lines 533-573) asserts only status, error code and that the message names the member — it never inspects `fieldList`. The only fieldList assertion in the suite (line 522, inside a_well_formed_non_cursor_next_token_is_refused_with_the_pagination_token_message) asserts fieldList is ABSENT for a cursor-decode failure, which is a different case (no member constraint violated). tests/all/body_validation_refusals.rs asserts codes/statuses only. No test covers fieldList for a member-constraint refusal, and no existing open/fixed uxc issue mentions it (the fixed ones cover the error CODE for body refusals, the pagination-token message, and the anchored-pattern rendering).

## Steps to Reproduce

1. Boot the emulator with the uxc service mounted.
2. GET /v1/services?maxResults=0 (or maxResults=1001, or nextToken=abc) — the request violates a modelled member constraint on a @httpQuery-bound input.
3. Observe HTTP 400 with x-amzn-errortype ValidationException and a body carrying only `message`.
4. Expected: the body additionally carries `fieldList: [{"path": "maxResults", "message": "<the constraint requirement>"}]` — one entry per violated member (repeat with both maxResults and a malformed nextToken in one request to see multiple entries).
5. Actual: `fieldList` is absent from every refusal this service renders.
