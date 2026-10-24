Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# CancelSigningProfile answers a 400 ValidationException the operation does not model when the profile is Revoked

_Filed: 2026-10-24T15:57:39Z_
_Operation: CancelSigningProfile | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CancelSigningProfile` (DELETE /signing-profiles/{profileName}) models exactly four failure modes: `AccessDeniedException`/403, `InternalServiceErrorException`/500, `ResourceNotFoundException`/404 and `TooManyRequestsException`/429. There is no 400 in its error set, and the AWS API reference for the operation documents the same four.

The emulator adds a fifth by hand: src/ops/cancel_signing_profile.rs declares `extra_errors = ["ValidationException"]` and, when the stored profile's status is `Revoked`, returns HTTP 400 with shape `ValidationException` and `code: "ProfileRevoked"`. For a client/SDK this is an UNMODELED error on this operation: the generated error union for CancelSigningProfile cannot carry `ValidationException`, so the call surfaces as a generic/unknown service error rather than a typed one, and a 400 appears for an operation whose contract has none.

Expected: cancelling a profile whose status is `Revoked` either succeeds (the documented transition semantics — the record stays, only its status moves, which is already how a second cancel of an already-Canceled profile behaves here) or is refused with one of the four error shapes the operation actually models. Actual: a fabricated 400 `ValidationException` carrying a code (`ProfileRevoked`) that no modeled shape for this operation can carry.

## Evidence

Smithy model: `smithy_query(query="operation", operation="CancelSigningProfile")` lists errors AccessDeniedException/403, InternalServiceErrorException/500, ResourceNotFoundException/404, TooManyRequestsException/429 — no 400 and no ValidationException. Handler: services/service-com-amazonaws-signer-wallabyservice/src/ops/cancel_signing_profile.rs lines ~118-133 widen the error enum with `extra_errors = ["ValidationException"]`, and the `if profile.is_revoked()` branch returns it with HTTP 400. Test coverage: tests/all/error_responses.rs case "CancelSigningProfile on a revoked profile" PINS the current behavior (status 400, shape ValidationException, code ProfileRevoked), so the divergence is asserted rather than caught.

## Steps to Reproduce

1. PUT /signing-profiles/Revoked_Profile with {"platformId": "AWSLambda-SHA384-ECDSA"} — note the returned profileVersion.
2. PUT /signing-profiles/Revoked_Profile/revoke with {"profileVersion": "<that version>", "reason": "rotate-signing-key", "effectiveTime": 1700000000} — observe 204.
3. DELETE /signing-profiles/Revoked_Profile — observe HTTP 400 with body shape ValidationException and code ProfileRevoked, an error the operation's modeled error set does not contain (an SDK deserializes it as an unknown/unmodeled error).
4. For contrast: DELETE /signing-profiles/<a Canceled profile> answers 204 (idempotent), showing the 400 is specific to the Revoked status.
