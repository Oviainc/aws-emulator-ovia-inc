Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# ValidationException rendered for a modelled constraint violation omits the required `reason` member (and `fieldList`)

_Filed: 2026-10-24T16:46:19Z_
_Operation: ListReports | Class: error-response_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every boundary refusal of a URI-bound member is answered as 400 `ValidationException` with a body carrying only `__type` and `message`. The modelled shape requires more: `ValidationException` declares `message` @required AND `reason` (`ValidationExceptionReason`) @required, plus the optional `fieldList` ("The field that caused the error, if applicable").

Expected: 400 ValidationException whose body carries `reason` — `fieldValidationFailed` for a member whose value fell outside its modelled constraint — and, where the field is known, a `fieldList` entry naming the member and the requirement it failed. This is the shape the service itself produces on its handler-built refusals: `crate::state::invalid_token()` sets `reason: "invalidToken"`, `too_many_tags()` and `unknown_query_identifiers()` set `reason: "fieldValidationFailed"` with a `fieldList` entry.

Actual: the boundary path goes through `Service::override_refusal`, which can only return `Refused { code, message }` (vendor/smithy-server-runtime/src/pre_dispatch.rs), so the rendered JSON is `{"message": "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300"}` with no `reason` and no `fieldList`. A client that branches on `ValidationExceptionReason` (or an SDK that deserializes the error shape) sees a required member missing on the wire for every constraint refusal.

This is one class of refusal across many operations — the same body is produced for `maxResults` out of range on ListReports / ListReportVersions / ListCustomerAgreements / ListComplianceInquiries / ListComplianceInquiryQueries, `reportVersion` below its range on GetReportMetadata / GetReport / GetTermForReport, `nextToken` outside its length on every paginated operation, `termToken` outside its length or pattern on GetReport, and `tagKeys` / `resourceArn` violations on the tag operations. ListReports is the simplest reproduction. The status (400) and the code (ValidationException) are already correct and must not change; only the body is incomplete.

## Evidence

Spec: `com.amazonaws.artifact#ValidationException` = { message: String @required, reason: ValidationExceptionReason @required, fieldList: ValidationExceptionFieldList }, @error client, @httpError 400; `ValidationExceptionReason` values are unknownOperation | cannotParse | fieldValidationFailed | invalidToken | other. `ListReportsRequest$maxResults` is `@range(min:1, max:300)` @httpQuery, so the refusal is in-contract.

Handler/seam: services/service-com-amazonaws-artifact-artifact/src/service.rs `override_refusal` returns `Some(Refused { code: "ValidationException", message: crate::state::constraint_violation_message(violations)? })` — a message-only answer; `Refused` (vendor/smithy-server-runtime/src/pre_dispatch.rs:164) carries no structured members, and `protocol_error_response_with` renders only `__type`/`message` for restJson1. Compare src/state.rs `invalid_token()` / `too_many_tags()` / `unknown_query_identifiers()`, which all populate `reason` (and `fieldList` where a field is named) on the handler-built ValidationExceptions — the asymmetry is in the code.

Tests: tests/all/validation_message_rendering.rs asserts status, `__type` and `message` for the ten boundary-constraint rows (lines ~140-267) and never inspects `reason`; the only `reason` assertion in the suite is for the handler-built invalid-token path (lines ~400-405, expecting `invalidToken`). So no test asserts the required member for the boundary path, and no existing issue covers it: the fixed issue 20261022T012757Z cause 6 addressed only the MESSAGE wording (dropping the echoed value) for this same refusal.

## Steps to Reproduce

1. Boot the emulator and send GET /v1/report/list?maxResults=301 signed for the artifact service.
2. Observe 400 with x-amzn-errortype ValidationException and body {"message":"1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300"} — no `reason`, no `fieldList`.
3. Expect the modelled shape instead: the same status, code and message plus `reason` = "fieldValidationFailed" (and, where the member is known, a fieldList entry naming `maxResults` and the requirement it failed).
4. Repeat with GET /v1/report/getMetadata?reportId=<a report id from ListReports>&reportVersion=0, GET /v1/report/list?nextToken=<2049 chars> and GET /v1/report/get?reportId=<id>&termToken=token.with.dots: every boundary refusal shows the same missing required member.
5. For contrast send GET /v1/report/list?nextToken=nota (a well-formed but never-issued token): the handler-built refusal does carry "reason":"invalidToken", showing the gap is confined to the boundary-refusal rendering.
