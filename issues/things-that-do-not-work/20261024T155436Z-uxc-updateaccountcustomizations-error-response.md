Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# A request body the codec cannot read (JSON type mismatch / malformed JSON) escapes the front door and answers a framework error code instead of BadRequestException

_Filed: 2026-10-24T15:54:36Z_
_Operation: UpdateAccountCustomizations | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service's front door is recorded answering HTTP 400 with error code `BadRequestException` and the body-level message "Invalid request body" for a PATCH /v1/account-customizations body it will not act on (that is the behavior the now-fixed issue 20261022T041007Z pinned from the traces uxc.visible-service-invalid-pattern-rejected, uxc.visible-services-exceed-max-size-rejected, uxc.invalid-account-color-enum-value-rejected, and what tests/all/body_validation_refusals.rs now asserts). The emulator reproduces that code ONLY for bodies that parse into the input shape and then break a modelled constraint.

A body that the codec cannot read into `UpdateAccountCustomizationsInput` at all — a member sent with the wrong JSON type (`{"visibleServices": "s3"}`, `{"visibleServices": {"a": 1}}`, `{"accountColor": 7}`, `{"visibleRegions": [1, 2]}`) or a syntactically malformed JSON document — takes a different path: `FrontDoor::run` calls `body_violations::<O>`, which returns an EMPTY violation list whenever `deserialize_input` or `DeserializableStruct::deserialize` fails, so the front door does not refuse it and the request falls through to the framework's own walk.

Expected: the same class of answer the front door gives any other body this service refuses — HTTP 400, error code `BadRequestException` in both `x-amzn-errortype` and the body's `__type`, with a body-level message that names no member.

Actual: whatever generic code the framework's deserialization path renders for this restJson1 service — per the comment in src/service.rs the intent is the modelled `ValidationException`, and the framework default used by sibling services for an unreadable body/value is `SerializationException` (e.g. services/service-com-amazonaws-xray-awsxray/src/service/request_layer.rs). Either way it is not the `BadRequestException` a client of this service branches on, so error-path handling for a malformed PATCH body diverges exactly as it did before 20261022T041007Z was fixed — that fix closed only the constraint-violation half.

## Evidence

Spec/recording: the model declares only `AccessDeniedException` (403), `InternalServerException` (500), `ThrottlingException` (429) and `ValidationException` (400) for UpdateAccountCustomizations (smithy_query query="operation"), yet the recorded AWS answer for a malformed body is 400 `BadRequestException` / "Invalid request body" — documented and accepted in the fixed issue 20261022T041007Z-uxc-updateaccountcustomizations-wrong-error-code-for-body-validation-refusals (Cause 1/Cause 2, traces listed there), and now encoded as the emulator's own rule in src/service.rs lines 113-127 and tests/all/body_boundary_refusal.rs.

Handler: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/service.rs lines 247-269 — `body_violations` returns `Vec::new()` on `deserialize_input` failure and on `DeserializableStruct::deserialize` failure ("a codec failure is not a constraint violation, so it is left to the framework's own walk"), so `front_door_rejection` (lines 299-307, the only place `BAD_REQUEST_EXCEPTION` is emitted) never fires for an unreadable body. `override_refusal` (lines 342-380) only handles `Refusal::Invalid` constraint violations and always labels them `ValidationException`.

Tests: no test in tests/all sends a wrong-typed or syntactically malformed body. tests/all/body_validation_refusals.rs and tests/all/body_boundary_refusal.rs only use well-typed JSON that violates a modelled enum/pattern/length bound; tests/all/account_customizations.rs covers accepted bodies. So this path is uncovered, and no open or fixed uxc issue mentions the codec-failure case (the open ones are the null-reset, catalog-membership and ListServices fieldList issues).

## Steps to Reproduce

1. Boot the emulator with the uxc service mounted.
2. PATCH /v1/account-customizations with body {"visibleServices": "s3"} — a member whose JSON type does not match the modelled list, so the body cannot be read into the input shape.
3. Observe the response code in x-amzn-errortype and in the body's __type: a framework code (ValidationException / SerializationException), not BadRequestException.
4. Repeat with {"accountColor": 7}, {"visibleRegions": [1,2]} and a syntactically malformed document (e.g. {"accountColor": ) for the same divergence.
5. Compare against PATCH with {"accountColor": "chartreuse"} (a readable body breaking a modelled constraint), which correctly answers 400 BadRequestException / "Invalid request body" — showing the two malformed-body classes answer different codes in the emulator where the service answers one.
6. Confirm nothing was persisted in either case via GET /v1/account-customizations.
