Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 68144f2b1a9e2dba602611483c97527f7042e8b8

# A missing @required member short-circuits the pre-dispatch walk ahead of target authorization, so a malformed reportId answers 400 instead of the front door's 401

_Filed: 2026-10-28T22:23:29Z_
_Operation: GetReport | Class: error-response_

> _Note: this issue was identified against the code base at commit `68144f2b1a9e2dba602611483c97527f7042e8b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EmulatedService::judge` (src/service.rs) is the one place this crate layers the front door's two refusals against the model's own constraints, and it states the order explicitly: the target is authorized FIRST, ahead of every constraint the model declares, and only a request that passes that is refused for what its URI or body carried. The code breaks that order on exactly one path: when the input cannot be DESERIALIZED because a `@required` member is absent, `judge` returns a refusal immediately — before step 1 ever runs — so the answer is decided by the absent member rather than by the unauthorizable target the request named.

Consequence on `GetReport`, whose `termToken` is `@required` and `@httpQuery`:

* `GET /v1/report/get?reportId=report-abc&termToken=never-issued` answers 401 `UnauthorizedException` (the id is not a well-formed `ReportId`, so it names no report to authorize) — this is what `tests/all/front_door_errors.rs` pins.
* `GET /v1/report/get?reportId=report-abc` (the same unauthorizable target, `termToken` simply left out) answers 400 `ValidationException` with `reason=fieldValidationFailed` and a `fieldList` entry naming `termToken` — the modelled constraint the crate's own rule says the 401 precedes.

The same hole exists on `PutComplianceInquiryFeedback`, where the missing member is body-bound: a malformed `complianceInquiryId` with `rating` omitted answers 400 `BadRequestException` instead of the 401 the same request with `rating` present answers. The root cause is one early return in the shared `judge` helper, so both symptoms are the same defect.

Note this is the residual hole in the ordering the FIXED issue `20261022T003730Z-artifact-error-responses-return-wrong-http-status-and` installed (its cause 4 established that a malformed `complianceInquiryId` must answer 401 ahead of the modelled pattern constraint); nothing here asks for that fix to be reversed — absence of a required member should be judged after the target, exactly as a present-but-invalid value already is.

## Evidence

The order the crate states for itself, in the doc of the very function that breaks it — `src/service.rs:253-255`:

    /// The layering decides the order: the gateway answers ahead of this service, so an
    /// unauthorizable target comes first and a body it cannot bind second, and only a request
    /// both of them pass is refused for a value its URI carried.

and restated as the numbered step at `src/service.rs:289`:

    // 1. Authorize the target, ahead of every constraint the model declares.

The code that answers before that step, `src/service.rs:266-287`:

    let input = match I::deserialize(&mut *deserializer) {
        Ok(input) => input,
        // A member the shape requires, absent from the body, is a body the gateway
        // cannot bind. A URI-bound one is this service's own refusal, stated against the
        // member the request did not carry — absence is a constraint violation like any
        // other, found while reading the request rather than while walking it.
        Err(aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => {
            return Some(if body_bound(schema, &member_name) {
                PreDispatchRefusal::FrontDoor(FrontDoorVerdict::InvalidRequestBody)
            } else {
                PreDispatchRefusal::InvalidInput(vec![Violation {
                    path: member_name,
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                }])
            })
        }
        Err(_) => return None,
    };

The comment inside that arm classifies absence as "a constraint violation like any other" — and `src/service.rs:289` says the target is authorized "ahead of every constraint the model declares", so by the crate's own two sentences this return is in the wrong place.

The target check it jumps, `src/service.rs:290-292`:

    if target(&input).is_some_and(|target| !target.names_a_resource()) {
        return Some(PreDispatchRefusal::FrontDoor(FrontDoorVerdict::Unauthorized));
    }

`GetReport` is routed through that helper with its report target at `src/service.rs:669-676` (`|input| Some(Target::Report(&input.report_id))`), and `termToken` is `@required` + `@httpQuery` in the model (`smithy_query(query="operation",operation="GetReport")`: `"termToken": {"target": "ShortStringAttribute", "httpQuery": "termToken", "required": true, ...}`), so `GetReportInput.term_token: String` (src/ops/types/get_report_types.rs:19) is the member whose absence raises `MissingMember`.

Test coverage — the existing suites always supply the other required member, so none of them pins the broken combination, and one states the rule the code breaks:

* `tests/all/front_door_errors.rs:240-246` — the pinned `GetReport` row keeps `termToken` present: `("GetReport", format!("/v1/report/get?reportId={id}&termToken=never-issued"))` … `assert_refused(&format!("{operation} {label}"), &answer, UNAUTHORIZED);` over `MALFORMED_IDS`.
* `tests/all/front_door_errors.rs:256-258` — "The identifier is bound to the query string on two operations and to the request body on the other two; the gateway authorizes the target either way, and does so BEFORE it validates that body." The `PutComplianceInquiryFeedback` row under it carries `"rating": "THUMBS_UP"` (`front_door_errors.rs:292`), so the missing-`rating` case is untested.
* `tests/all/report_workflow.rs:767-813` sends its malformed-id rows only to `GetReportMetadata` and `GetTermForReport`, neither of which has a second `@required` member, so they cannot reach this path either.

## Steps to Reproduce

1. Boot the emulator from this checkout with the artifact service mounted (as tests/all/harness.rs does).
2. GET /v1/report/get?reportId=report-abc&termToken=never-issued — observe HTTP 401 with x-amzn-errortype: UnauthorizedException, the answer tests/all/front_door_errors.rs:240-246 pins for an id that is not a well-formed ReportId.
3. GET /v1/report/get?reportId=report-abc (same unauthorizable target, termToken omitted) — observe HTTP 400 with x-amzn-errortype: ValidationException, reason fieldValidationFailed and a fieldList entry naming termToken, instead of the 401 from step 2.
4. PUT /v1/compliance-inquiry/putFeedback with body {"complianceInquiryId":"not-a-valid-id","rating":"THUMBS_UP"} — observe HTTP 401 UnauthorizedException (front_door_errors.rs:286-298).
5. PUT /v1/compliance-inquiry/putFeedback with body {"complianceInquiryId":"not-a-valid-id"} (rating omitted) — observe HTTP 400 BadRequestException instead, although the target is the same unauthorizable one.
