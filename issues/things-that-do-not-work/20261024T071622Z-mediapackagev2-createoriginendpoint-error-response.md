Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Too many manifests is answered with 402 ServiceQuotaExceededException instead of 400 ValidationException / NUM_MANIFESTS_HIGH

_Filed: 2026-10-24T07:16:22Z_
_Operation: CreateOriginEndpoint | Class: error-response_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator treats the manifest-count floor and ceiling as two different error classes. Too few manifests goes through crate::state::manifest_set_violation and is answered, correctly, with 400 ValidationException and ValidationExceptionType NUM_MANIFESTS_LOW. Too many goes through crate::state::manifest_ceiling_exceeded and is answered with 402 ServiceQuotaExceededException and a message about the emulator's own ceiling ("An origin endpoint accepts at most 100 manifests; the request has N").

The model names NUM_MANIFESTS_HIGH right beside NUM_MANIFESTS_LOW in ValidationExceptionType, i.e. AWS reports an over-sized manifest set as a 400 ValidationException with that discriminator, not as a quota error. Expected: exceeding the per-endpoint manifest ceiling is 400 ValidationException with ValidationExceptionType NUM_MANIFESTS_HIGH (the numeric ceiling itself may remain the emulator's choice). Actual: 402 with no ValidationExceptionType, so a client branching on ValidationException/NUM_MANIFESTS_HIGH sees a retryable-looking quota error instead. UpdateOriginEndpoint uses the same helper (src/ops/update_origin_endpoint.rs lines 381-390) and diverges the same way.

## Evidence

Spec: ValidationExceptionType includes both NUM_MANIFESTS_LOW and NUM_MANIFESTS_HIGH; CreateOriginEndpoint declares ValidationException (400) and ServiceQuotaExceededException (402) separately, and the HIGH discriminator exists only on the validation error.
Handler: src/ops/create_origin_endpoint.rs lines 420-429 maps crate::state::manifest_ceiling_exceeded to ServiceQuotaExceededException; src/state.rs lines 2358-2377 documents the choice ("the refusal is the ServiceQuotaExceededException the counting ceilings use") while lines 2319-2334 answer the LOW case as ValidationException/NUM_MANIFESTS_LOW.
Tests: no test asserts the over-count class (grep of tests/ finds NUM_MANIFESTS_LOW assertions only via control_plane_validation.rs; nothing for NUM_MANIFESTS_HIGH).

## Steps to Reproduce

1. Create a channel group and a channel.
2. CreateOriginEndpoint with ContainerType CMAF and 101 HlsManifests entries with distinct ManifestNames.
3. Observe HTTP 402 with error code ServiceQuotaExceededException and no ValidationExceptionType; AWS answers 400 ValidationException with ValidationExceptionType NUM_MANIFESTS_HIGH.
4. Repeat through UpdateOriginEndpoint on an existing endpoint — same 402.
