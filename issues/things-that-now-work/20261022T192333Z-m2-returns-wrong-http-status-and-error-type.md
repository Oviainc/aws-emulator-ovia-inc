Issue-Source: trace-sweep-divergence
Issue-Trace: m2-create-resource-access-denied
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: m2-create-resource-access-denied m2-engine-discovery-and-blu-insights m2-environment-not-found-errors
Issue-Shape-Id: com.amazonaws.m2#AwsSupernovaControlPlaneService
Issue-Key: m2|CreateApplication|4xx->2xx|$.message
Issue-Key: m2|CreateApplication|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: m2|CreateApplication|4xx->2xx|status
Issue-Key: m2|GetSignedBluinsightsUrl|2xx->5xx|$.signedBiUrl
Issue-Key: m2|GetSignedBluinsightsUrl|2xx->5xx|status
Issue-Key: m2|ListTagsForResource|4xx->4xx|$.message
Issue-Key: m2|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# m2 returns wrong HTTP status and error type for unauthorized, unimplemented and malformed-ARN requests

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Three independent causes in the m2 (AWS Mainframe Modernization, restJson1) emulator where the HTTP status and/or the x-amzn-errortype of the response differ from what the service returns. Status codes and error codes are wire requirements, so each of these makes a correct client take the wrong branch. All three must be closed before this issue is submitted.

Cause 1 of 3: CreateApplication answers 200 for a request the service refuses with AccessDeniedException.
AWS: CreateApplication (POST /applications) binds AccessDeniedException among the service-common errors (Smithy: AwsSupernovaControlPlaneService common_errors = AccessDeniedException/InternalServerException/ThrottlingException; AccessDeniedException @error client, @httpError 403, required member `message`). The recorded response for this step is 403 with x-amzn-errortype: AccessDeniedException and message 'Unable to determine service/operation name to be authorized' (recorded behavior only for the text; Hypothesis: the recorded request is shaped so the service cannot map it to an authorizable service/operation pair, and the authorization layer rejects it before the handler runs).
Emulator: returns 200 with a CreateApplication success body — no 403, no message member, no x-amzn-errortype header at all. The application is created as a side effect.
Why: a client exercising a deny path sees a created resource instead of AccessDeniedException. Nothing raises the modelled 403, so permission/authorization handling cannot be tested, and the emulator's state now contains an application that AWS would never have created, which skews every later step of the fixture.
Fix: the m2 CreateApplication request path — the pre-handler authorization/request-identification stage that must emit AccessDeniedException (403, x-amzn-errortype AccessDeniedException, `message` member) before any state mutation.
Traces: m2-create-resource-access-denied#task#2.

Cause 2 of 3: GetSignedBluinsightsUrl is not served — 501 instead of the modelled 200.
AWS: GetSignedBluinsightsUrl (Smithy: GET /signed-bi-url, input Unit, output GetSignedBluinsightsUrlResponse with required member `signedBiUrl`; errors AccessDeniedException/InternalServerException/ThrottlingException — 501 is not a modelled outcome). The recording is a 200 carrying a signedBiUrl string (its value is environment-specific and was not asserted).
Emulator: 501, and the response carries no `signedBiUrl` member.
Why: the operation is unusable; an SDK call fails with an unmodelled 501 that no client retry/error mapping handles, instead of receiving the required signedBiUrl member.
Fix: the m2 GetSignedBluinsightsUrl handler — implement it to return 200 with the required `signedBiUrl` member (a deterministic emulator-local URL is sufficient; its exact value is not a requirement).
Traces: m2-engine-discovery-and-blu-insights#task#4.

Cause 3 of 3: a syntactically invalid resourceArn on ListTagsForResource is refused with the wrong error type.
AWS: ListTagsForResource (Smithy: GET /tags/{resourceArn}, resourceArn @httpLabel @required, Arn @pattern ^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):...). For the value 'not-a-valid-arn' the recorded response is 400 with x-amzn-errortype: BadRequestException and message 'Invalid input resource arn: bad syntax' — i.e. the ARN is parsed by the tagging/ARN layer and rejected as a bad request, not reported as a Smithy constraint violation (BadRequestException is not in the operation's modelled error list, so this is recorded behavior only).
Emulator: 400 but x-amzn-errortype: ValidationException, with the generic constraint text "1 validation error detected: Value 'not-a-valid-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:...".
Why: the status matches but the error code does not. SDK error mapping and customer catch-blocks dispatch on x-amzn-errortype, so code that handles BadRequestException (or distinguishes it from ValidationException) takes the wrong branch against the emulator.
Fix: the m2 ListTagsForResource (and sibling tag operations') resourceArn handling — ARN syntax rejection must be raised as BadRequestException 400 rather than falling through to the generic pattern-constraint ValidationException serializer.
Traces: m2-environment-not-found-errors#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, bootstrap_signal false, state_reset_failures 0; 36 steps, 2 passed, 34 mismatched, 8 divergence clusters — all service=m2).

Cause 1 — fixture m2-create-resource-access-denied, step task#2, CreateApplication:
  expected status 403, actual 200
  $.message: missing (expected 'Unable to determine service/operation name to be authorized', got None)
  $headers.x-amzn-errortype: missing (expected 'AccessDeniedException', got None)

Cause 2 — fixture m2-engine-discovery-and-blu-insights, step task#4, GetSignedBluinsightsUrl:
  expected status 200, actual 501
  $.signedBiUrl: missing (expected a shaped string, got None)

Cause 3 — fixture m2-environment-not-found-errors, step task#4, ListTagsForResource:
  status 400 both sides
  $headers.x-amzn-errortype: expected 'BadRequestException', got 'ValidationException'
  $.message: expected 'Invalid input resource arn: bad syntax', got "1 validation error detected: Value 'not-a-valid-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):[A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:..."

Model evidence (services/service-com-amazonaws-m2-awssupernovacontrolplaneservice/smithy-model/com-amazonaws-m2-awssupernovacontrolplaneservice.json): sdkId m2, protocol restJson1; common_errors AccessDeniedException(403)/InternalServerException(500)/ThrottlingException(429); GetSignedBluinsightsUrl GET /signed-bi-url -> 200 GetSignedBluinsightsUrlResponse{signedBiUrl required}; ListTagsForResource GET /tags/{resourceArn}, resourceArn targets Arn with the ARN @pattern; CreateApplication POST /applications.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture m2-create-resource-access-denied step task#2 — POST /applications with the fixture's recorded (unauthorizable) request. Expect 403 + x-amzn-errortype: AccessDeniedException + a `message` member; observe 200 and a created application.
3. Cause 2: GET /signed-bi-url (fixture m2-engine-discovery-and-blu-insights step task#4). Expect 200 with a `signedBiUrl` member; observe 501.
4. Cause 3: GET /tags/not-a-valid-arn (fixture m2-environment-not-found-errors step task#4). Expect 400 with x-amzn-errortype: BadRequestException; observe 400 with x-amzn-errortype: ValidationException.

## Resolution

Cause 2 and cause 3 are FIXED, and so is the general rule cause 1 names; two parts of
cause 1's recorded instance are DECLINED and are recorded here so a later sweep does
not have to re-derive them.

- Cause 2 — `GetSignedBluinsightsUrl` is served: `src/ops/get_signed_bluinsights_url.rs`
  (hand-authored, the generator declines a `smithy.api#Unit` input) answers 200 with
  `signedBiUrl` built on the endpoint the request arrived on, naming the account and
  the signed region. Covered by `tests/all/signed_bluinsights_url.rs`.
- Cause 3 — a `resourceArn` that `service_common::arn::parse` does not read as an ARN
  is refused 400 `BadRequestException` "Invalid input resource arn: bad syntax"
  (`src/service.rs`'s `override_refusal`); an ARN-shaped value outside the modelled
  `Arn` pattern, and every other `Arn`-typed member, keep the modelled
  `ValidationException`. Covered by `tests/all/pre_handler_refusals.rs`.
- Cause 1, the RULE — a caller the pre-handler authorization stage refuses is now
  answered with the modelled `AccessDeniedException` (403, `x-amzn-errortype:
  AccessDeniedException`, a `message` member) instead of the framework's `AccessDenied`,
  before any state mutation. Covered by `tests/all/pre_handler_refusals.rs`.
- DECLINED, `m2|CreateApplication|4xx->2xx|{status,$.message,$headers.x-amzn-errortype}`:
  the recorded request was read off the wire and is an ordinary, fully conforming
  `POST /applications` (name/engineType/definition/clientToken, `application/json`,
  botocore headers) on exactly the URI the model binds CreateApplication to, re-signed
  by the replay harness with the emulator's own credentials. Nothing in it distinguishes
  it from an authorized create, and the recorded message ("Unable to determine
  service/operation name to be authorized") is AWS's authorization layer failing to name
  a service/operation pair — a property of the recording principal/endpoint, not of the
  request. No AWS rule produces that answer for that request, so the emulator still
  answers 200 here and will keep doing so.
- DECLINED, `m2|GetSignedBluinsightsUrl|…|$.signedBiUrl` (the VALUE, once the status is
  right): a single-sign-on URL AWS generates per call, recorded as environment-specific
  and not asserted; the model constrains it only as a `String`.
