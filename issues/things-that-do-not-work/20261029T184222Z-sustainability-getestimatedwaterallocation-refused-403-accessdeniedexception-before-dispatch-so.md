Issue-Source: trace-sweep-divergence
Issue-Trace: sustainability-water-allocation-invalid-allocation-type-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sustainability-water-allocation-invalid-allocation-type-rejected
Issue-Shape-Id: com.amazonaws.sustainability#AwsSustainabilityApiService
Issue-Key: sustainability|GetEstimatedWaterAllocation|4xx->4xx|$.message
Issue-Key: sustainability|GetEstimatedWaterAllocation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sustainability|GetEstimatedWaterAllocation|4xx->4xx|status
Issue-Key: sustainability|GetEstimatedWaterAllocation|2xx->4xx|$.Results
Issue-Key: sustainability|GetEstimatedWaterAllocation|2xx->4xx|status
Issue-Commit: 1f4ddd4c47b0507bb1e50344cb9c3bb105203334

# sustainability GetEstimatedWaterAllocation refused 403 AccessDeniedException before dispatch, so neither its validation refusal nor its 200 answer is reachable

> _Note: this issue was identified against the code base at commit `1f4ddd4c47b0507bb1e50344cb9c3bb105203334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEstimatedWaterAllocation

## Description
2 causes, both on GetEstimatedWaterAllocation and both produced by the same pre-dispatch refusal: every request to this service is answered 403 AccessDeniedException ("... Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256).") before any operation handler runs, so the operation's modelled refusal AND its modelled success answer are both unreachable. The service's model lists aws.auth#sigv4a FIRST (Smithy: AwsSustainabilityApiService authSchemes [sigv4, sigv4a], protocol restJson1), so an SDK configured for it signs with AWS4-ECDSA-P256-SHA256; the request IS claimed by the service (services/service-com-amazonaws-sustainability-awssustainabilityapiservice/src/service.rs can_handle widens the claim to any SigV4-family credential scope), but the authentication gate then treats it as Refusal::Unauthenticated, which this service's override_refusal words as the modelled AccessDeniedException. Note: three earlier sustainability issues on the 403/UnknownService family are RESOLVED (20261022T174638Z-sustainability-every-request-is-refused-with-unknownservice-the.md, 20261022T200433Z-sustainability-getestimatedwaterallocation-rejected-with-403-accessdenied-instead-of-being.md, 20261022T232611Z-sustainability-all-4-operations-refused-403-accessdenied-sign.md); the corpus still diverges the same way, and a resolved issue covers nothing.

Cause 1 of 2 — an invalid AllocationTypes enum member is answered 403 AccessDeniedException instead of the modelled 400 ValidationException (fixture sustainability-water-allocation-invalid-allocation-type-rejected, step task#1).
AWS: POST /v1/estimated-water-allocation with AllocationTypes containing a value outside the WaterAllocationType enum is refused with ValidationException (Smithy: GetEstimatedWaterAllocation errors include ValidationException @error("client") @httpError(400), required Message with jsonName "message"; WaterAllocationTypeList member -> enum WaterAllocationType whose only value is TOTAL_WATER_WITHDRAWALS). The recorded response is HTTP 400 with the AWS constraint-violation envelope: "1 validation error detected: Value '[<supplied member values>]' at 'allocationTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [TOTAL_WATER_WITHDRAWALS]]". Fixed text of that class: "<n> validation error(s) detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [", "]]"; the variable parts are the violation count, the rejected value list taken from the request, the input member path (allocationTypes), and the enum value set taken from the model. x-amzn-errortype must name a shape this model declares (ValidationException).
Emulator: HTTP 403 with x-amzn-errortype: AccessDeniedException and message "You do not have sufficient access to perform this action. Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256)." — the request never reaches input validation.
Why: a client that branches on ValidationException to report a bad enum value instead sees an auth failure; it will re-sign or re-credential rather than fix the request, and the member path/enum value set the error is supposed to carry is lost.
Fix: the service's request-authentication gate — accept the SigV4A signing flavour (AWS4-ECDSA-P256-SHA256, whose credential scope omits the region segment) that this model's first auth scheme implies, so the request reaches the operation's constrained-input validation; the ValidationException rendering itself is already the framework's.
Traces: sustainability-water-allocation-invalid-allocation-type-rejected#task#1.

Cause 2 of 2 — a valid GetEstimatedWaterAllocation request is answered 403 AccessDeniedException instead of 200 with its required Results list (fixture sustainability-water-allocation-invalid-allocation-type-rejected, step task#2).
AWS: GetEstimatedWaterAllocation is @readonly and answers HTTP 200 with GetEstimatedWaterAllocationResponse, whose Results (EstimatedWaterAllocationList) is REQUIRED and is present-but-empty when the request's TimePeriod/filters select no rows; NextToken is optional (Smithy: GetEstimatedWaterAllocation output Results required, http code 200).
Emulator: HTTP 403 AccessDeniedException with no body members at all — $.Results is missing where an empty list is required.
Why: Results is a required output member, so a generated SDK client cannot deserialize the answer and an "empty result set" is indistinguishable from an authorization failure; every downstream read of the water-allocation surface is unreachable.
Fix: same gate as Cause 1 (the service's authentication/credential acceptance ahead of dispatch); once the request dispatches, the handler must still serialize Results as an empty list rather than omitting it.
Traces: sustainability-water-allocation-invalid-allocation-type-rejected#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up: /__admin/health 200, data listener :40091 accepting). 2 of 2 steps mismatched, score 0.0%.

Step task#1 (fixture sustainability-water-allocation-invalid-allocation-type-rejected), GetEstimatedWaterAllocation:
  status: expected 400, got 403
  $.message: expected "1 validation error detected: Value '[NOT_A_REAL_ALLOCATION_TYPE]' at 'allocationTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [TOTAL_WATER_WITHDRAWALS]]", got "You do not have sufficient access to perform this action. Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256)."
  $headers.x-amzn-errortype: expected a shape of this model's error set, got AccessDeniedException

Step task#2 (same fixture), GetEstimatedWaterAllocation:
  status: expected 200, got 403
  $.Results: expected [] (required output member), got absent

Model evidence: authSchemes [sigv4, sigv4a] with sigv4a listed first; WaterAllocationType enum = [TOTAL_WATER_WITHDRAWALS]; ValidationException @httpError(400); AccessDeniedException @httpError(403); output member Results required.
Code evidence: services/service-com-amazonaws-sustainability-awssustainabilityapiservice/src/service.rs lines 169-197 (can_handle claims SigV4-family scopes) and lines 225-236 (Refusal::Unauthenticated -> AccessDeniedException), i.e. the request is claimed and then refused before dispatch.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout and replay fixture sustainability-water-allocation-invalid-allocation-type-rejected.
2. Step task#1: POST /v1/estimated-water-allocation to the sustainability endpoint, signed as an SDK configured for this model signs it (SigV4A, AWS4-ECDSA-P256-SHA256), with a body whose AllocationTypes contains a value outside the WaterAllocationType enum. Expect 400 ValidationException with the "N validation error(s) detected: Value '[...]' at 'allocationTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [TOTAL_WATER_WITHDRAWALS]]" envelope; observe 403 AccessDeniedException.
3. Step task#2: POST the same URI with a valid body (required TimePeriod, Granularity YEARLY_CALENDAR). Expect 200 with Results present (empty list when nothing is selected); observe 403 AccessDeniedException with no Results member.
