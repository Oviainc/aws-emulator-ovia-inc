Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Requests refused before the handler answer with content-type application/x-amz-json-1.0 instead of the service's 1.1

_Filed: 2026-10-24T15:28:43Z_
_Operation: UpgradeAccountPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service deliberately answers in the awsJson 1.1 envelope: every recorded response of the real AWSFreeTierService — HTTP 200 and HTTP 400 alike — carries `content-type: application/x-amz-json-1.1`, and `src/service.rs` implements `override_protocol()` to serve that codec. That override only reaches responses the SERVICE produces (a handler's success, and a modelled failure a handler raises). A request the framework refuses BEFORE `handle` runs is rendered by the pre-dispatch path, which uses the model-derived media type, so it answers `application/x-amz-json-1.0`.

Expected: `POST /` with `X-Amz-Target: AWSFreeTierService.UpgradeAccountPlan` and body `{}` (the @required `accountPlanType` absent) → HTTP 400, `__type: ValidationException`, `content-type: application/x-amz-json-1.1` — the same envelope every other response of this service uses.

Actual: HTTP 400 with the right status, code and message, but `content-type: application/x-amz-json-1.0`.

The status/code/body are correct on this path; only the announced media type disagrees, and it disagrees with every other response of the same service, so a client that keys its parsing or protocol assertion off the media type sees two different protocols from one service. The same divergence appears on every framework-refused request of this service, not just this one operation — consolidate them: `UpgradeAccountPlan {"accountPlanType":"GOLD"}` (EnumTrait violation), `ListAccountActivities {"maxResults": 5000}` and `GetFreeTierUsage {"maxResults": 0}` (@range 1..1000 violation), `GetAccountActivity {"activityId":"short"}` (@length/@pattern violation) and `GetAccountActivity {}` (absent required member) are all refused in front of the service and all announce 1.0.

## Evidence

Recorded wire behavior: issues/things-that-now-work/20261022T062051Z-freetier-responses-use-awsjson1-0-content-type-and.md records, from the simparity-wire-aws-v1 corpus, that "the recorded AWS responses for this service carry the response header content-type: application/x-amz-json-1.1, on success (HTTP 200) and on error (HTTP 400) alike — observed identically in every step of both fixtures". That issue's fix was partial: it covered only service-produced responses.

Emulator: services/service-com-amazonaws-freetier-awsfreetierservice/src/service.rs lines 46-59 and 137-144 — `override_protocol` returns the awsJson 1.1 codec and the module doc states outright that "the routing, transport and refusal envelopes keep the media type the model's protocol trait declares, which is the one seam override_protocol deliberately does not reach". vendor/smithy-server-runtime/src/pre_dispatch.rs lines 220-223 and 414-447 render a refusal with the per-service `content_type` taken from the generated (model-derived, awsJson1_0) definition, not from `override_protocol`; lines 304-308 show that path producing 400 `ValidationException` for `ProtocolError::Serde`/`Invalid`.

Spec: the input member `UpgradeAccountPlanRequest$accountPlanType` is @required (smithy_query query=operation operation=UpgradeAccountPlan), so an absent value is a documented failure mode of this operation, and `ValidationException` (@httpError 400) is modelled on it.

Test coverage: tests/all/wire_contract.rs `every_response_announces_the_services_media_type` has rows "an absent required member, refused in front of the service" (line 260-268) and "a target this service models no operation for" that PIN `expected_media_type: MODEL_MEDIA_TYPE` (1.0) — i.e. the existing test asserts the divergent form, so nothing asserts the recorded 1.1 on this path.

## Steps to Reproduce

1. Boot the emulator with the freetier service mounted.
2. POST / with `content-type: application/x-amz-json-1.0`, `X-Amz-Target: AWSFreeTierService.UpgradeAccountPlan` and body {"accountPlanType": "PAID"}; note the response header `content-type: application/x-amz-json-1.1`.
3. POST / with the same target and body {} (omitting the required `accountPlanType`); observe HTTP 400 ValidationException but `content-type: application/x-amz-json-1.0`.
4. Repeat with {"accountPlanType": "GOLD"} (value outside the AccountPlanType enum), with `AWSFreeTierService.ListAccountActivities` body {"maxResults": 5000} and with `AWSFreeTierService.GetAccountActivity` body {"activityId": "short"} — each is refused in front of the service and each announces 1.0.
5. Compare against a handler-raised failure, e.g. `AWSFreeTierService.GetFreeTierUsage` with {"filter": {"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS", "CONTAINS"]}}}: also HTTP 400 ValidationException, but announced as application/x-amz-json-1.1 — two media types from one service for the same class of refusal.
