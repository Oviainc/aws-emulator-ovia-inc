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
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# GetEstimatedWaterAllocation rejected with 403 AccessDenied instead of being handled

> **CLOSED WITH BOTH CAUSES DECLINED — THE REPLAYED SYMPTOM REMAINS.** This report sits
> under `things-that-now-work/` because it has been worked and needs no further fixer, not
> because the recorded fixture passes: a SigV4A-signed request is STILL answered 403 (now
> as the modelled `AccessDeniedException`). What the emulator owns was verified correct and
> pinned by test; what remains needs a change to the read-only core library, stated under
> "## Resolution" at the end of this file as **Required core change**. Re-open it there, not
> in this service.

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEstimatedWaterAllocation

## Description
Every replayed GetEstimatedWaterAllocation request against the sustainability emulator is answered 403 with x-amzn-errortype: AccessDenied and the body message 'Sign the request with the emulator credentials'. The response is identical for a request whose input violates a modeled enum constraint and for a well-formed request, which shows the rejection happens in the service's request-authentication path before the operation is dispatched: no input validation and no state lookup ever runs. 2 causes, both on the same operation.

Cause 1 of 2 (fixture sustainability-water-allocation-invalid-allocation-type-rejected, step task#1) — invalid AllocationTypes member must be a 400 ValidationException, not 403.
AWS: GetEstimatedWaterAllocation takes AllocationTypes: WaterAllocationTypeList, whose member targets the enum WaterAllocationType with exactly one permitted value, TOTAL_WATER_WITHDRAWALS (Smithy: services/service-com-amazonaws-sustainability-awssustainabilityapiservice/smithy-model/com-amazonaws-sustainability-awssustainabilityapiservice.json, shapes WaterAllocationTypeList / WaterAllocationType). The operation models ValidationException with @httpError 400 ('The input fails to satisfy the constraints specified by an Amazon Web Services service', required member Message, jsonName 'message'). A member value outside the enum set is therefore a client validation failure answered 400 with a message naming the offending member and the constraint it failed; the recording shows that message form as "1 validation error detected: Value '[NOT_A_REAL_ALLOCATION_TYPE]' at 'allocationTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [TOTAL_WATER_WITHDRAWALS]]" (an example of the text; the requirement is the 400 status, the ValidationException shape, and a message built from the rejected member and its enum constraint).
Emulator: status 403, header x-amzn-errortype: AccessDenied, body $.message = 'Sign the request with the emulator credentials'. Replay reports 'status: expected 400, got 403' plus mismatches on $.message and $headers.x-amzn-errortype.
Why: a client that sends an out-of-range enum value gets an authorization failure rather than a validation failure. SDK error handling branches on the modeled error shape, so the client surfaces AccessDeniedException and typically treats it as a credential/permission problem to retry or escalate, instead of surfacing a ValidationException that tells the caller which field to correct. Input-validation test suites against the emulator cannot distinguish a rejected value from a rejected caller.
Fix: the service's request-authentication / signature-verification layer must accept the emulator's standard signed request for this service so the operation is dispatched at all; then GetEstimatedWaterAllocation's input validation must enforce the WaterAllocationType enum on each AllocationTypes member and raise ValidationException (403 -> 400).
Traces: sustainability-water-allocation-invalid-allocation-type-rejected#task#1.

Cause 2 of 2 (fixture sustainability-water-allocation-invalid-allocation-type-rejected, step task#2) — well-formed request must return 200 with a Results list, not 403.
AWS: GetEstimatedWaterAllocation is @readonly, POST /v1/estimated-water-allocation, with a modeled success code of 200. Its output GetEstimatedWaterAllocationResponse has a REQUIRED member Results (EstimatedWaterAllocationList) plus optional NextToken, and the operation is @paginated on items=Results. A well-formed request with no matching data therefore succeeds with 200 and Results present as an empty list, not an error (Smithy: same model, operation GetEstimatedWaterAllocation; errors AccessDeniedException/403, ValidationException/400, ThrottlingException/429, InternalServerException/500).
Emulator: status 403 with x-amzn-errortype: AccessDenied; Results is absent entirely. Replay reports 'status: expected 200, got 403' and '$.Results: missing (expected [], got None)'.
Why: the operation is unusable. A caller never reaches the success path, so it cannot read an allocation estimate, cannot observe the required Results member, and cannot page with NextToken. Because the required output member is missing, a strict SDK also fails response deserialization rather than returning an empty result set, so 'no data' and 'not permitted' are indistinguishable.
Fix: same root component as Cause 1 — the sustainability request-authentication path must not reject correctly signed requests; GetEstimatedWaterAllocation's handler and serializer must then return 200 and always emit the required Results member (empty list when the query matches nothing).
Traces: sustainability-water-allocation-invalid-allocation-type-rejected#task#2.

No existing open issue references sustainability (list_existing_issues query 'sustainability' returned none), and neither divergence cluster carried a stub_path or an owned_by.

## Evidence
Replay summary: steps_total 2, steps_passed 0, steps_mismatched 2, score_pct 0.0, state_reset_failures 0, bootstrap_signal false, emulator_up true ('/__admin/health answered 200 and the :42309 data listener accepts'). Both steps are verdict=mismatch.

Step task#1 — expected_status 400, actual_status 403.
  detail: status: expected 400, got 403
          $.message: value_mismatch (expected "1 validation error detected: Value '[NOT_A_REAL_ALLOCATION_TYPE]' at 'allocationTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [TOTAL_WATER_WITHDRAWALS]]", got 'Sign the request with the emulator credentials')
          $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'AccessDenied')
  divergence_key: service=sustainability|GetEstimatedWaterAllocation|status=400->403|keys=$.message,$headers.x-amzn-errortype,status

Step task#2 — expected_status 200, actual_status 403.
  detail: status: expected 200, got 403
          $.Results: missing (expected [], got None)
  divergence_key: service=sustainability|GetEstimatedWaterAllocation|status=200->403|keys=$.Results,status

Model evidence (services/service-com-amazonaws-sustainability-awssustainabilityapiservice/smithy-model/com-amazonaws-sustainability-awssustainabilityapiservice.json):
  GetEstimatedWaterAllocation: http { method POST, uri /v1/estimated-water-allocation, code 200 }, readonly, paginated { items: Results, inputToken/outputToken NextToken, pageSize MaxResults }
  output GetEstimatedWaterAllocationResponse: Results (EstimatedWaterAllocationList) REQUIRED, NextToken optional
  errors: AccessDeniedException 403, ValidationException 400, ThrottlingException 429, InternalServerException 500
  WaterAllocationType enum values: [TOTAL_WATER_WITHDRAWALS]
  ValidationException: client error, httpError 400, required Message (jsonName 'message')

AccessDeniedException is a modeled error for this operation, so 403 is a legal status in general — but it is returned here for a correctly signed request, uniformly and independent of the request body, which is why it is a defect in the auth path rather than a per-input behavior.

## Steps to Reproduce
1. git_clone() the repo at branch mainline and boot the emulator built from that checkout (health endpoint /__admin/health, data listener on the replay port).
2. Replay the wire fixture sustainability-water-allocation-invalid-allocation-type-rejected from trace set simparity-wire-aws-v1 (replay_wire_corpus()).
3. Step task#1 POSTs GetEstimatedWaterAllocation to /v1/estimated-water-allocation with AllocationTypes containing the non-enum value NOT_A_REAL_ALLOCATION_TYPE. Expected: 400 ValidationException whose message names the 'allocationTypes' member and its enum constraint. Observed: 403, x-amzn-errortype: AccessDenied, message 'Sign the request with the emulator credentials'.
4. Step task#2 POSTs the same operation with a well-formed input. Expected: 200 with the required Results member present (empty list). Observed: 403 AccessDenied with no Results member.
5. Confirm the fix by re-replaying both steps of that fixture: task#1 must be 400/ValidationException and task#2 must be 200 with Results present.

## Resolution

Measured, not inferred: the replayed request is **SigV4A-signed**. A diagnostic run of the
fixture through this service's `override_refusal` reported the request's own
`Authorization` algorithm as `AWS4-ECDSA-P256-SHA256` (credential scope region-less, host
the emulator's loopback address) — which is what an SDK sends for this service, because
its model lists `aws.auth#sigv4a` AHEAD of `aws.auth#sigv4`
(`smithy.api#auth: ["aws.auth#sigv4a", "aws.auth#sigv4"]`; it is the only service in this
repo that orders them that way).

The emulator's ONLY request authenticator is `SigV4Authenticator` in
`vendor/server/src/request_authentication.rs`, whose `recognizes` accepts
`AWS4-HMAC-SHA256` alone and whose `verify` requires the five-field, region-bearing
credential scope; it is registered in `vendor/smithy-server-runtime/src/multi_service_router.rs`
(`MultiServiceRouter::new` / `set_request_authentication`). When no registered
authenticator recognizes a request, `authenticator_for` refuses it with `ACCESS_DENIED`
before the operation is ever named — uniformly, independent of the body, which is exactly
the symptom reported above. There is no service-level seam that can admit it: the auth
schemes come from the model via the generated `ErasedOperation::auth_schemes`, the
authenticator set is a private field of the router built in vendored code, and `vendor/`
is read-only here.

**Required core change** (serves every `aws.auth#sigv4a` service, not just this one): a
`RequestAuthenticator` for `aws.auth#sigv4a` in `vendor/server/src/request_authentication.rs`
— recognizing `AWS4-ECDSA-P256-SHA256`, reading the region-less
`<key>/<date>/<service>/aws4_request` scope plus `X-Amz-Region-Set`, and verifying the
ECDSA-P256 signature from the key pair SigV4A derives from the credential's secret —
registered beside the SigV4 one in `MultiServiceRouter::new`.

What was verified and fixed inside the emulator instead
(`services/service-com-amazonaws-sustainability-awssustainabilityapiservice/tests/all/signed_request_refusals.rs`,
which drives the `MultiServiceRouter` front door with request authentication ON and signs
each request with the emulator's own credentials):

* Cause 1's rule holds once the request is admitted: an `AllocationTypes` member outside
  `WaterAllocationType` — and every other value outside a declared value set or range,
  over all four routes — is answered `400 ValidationException` whose message is the AWS
  validation envelope naming the member and the constraint it failed.
* Cause 2's rule holds: a well-formed request is answered `200` with the required
  `Results` list always present, empty when the request selects nothing.
* A request this emulator cannot authenticate is now refused with the error SHAPE this
  model declares — `AccessDeniedException` (403) with its documented message — instead of
  the framework's `AccessDenied`, a code the model does not carry and which a `restJson1`
  client cannot resolve to a modelled error.
