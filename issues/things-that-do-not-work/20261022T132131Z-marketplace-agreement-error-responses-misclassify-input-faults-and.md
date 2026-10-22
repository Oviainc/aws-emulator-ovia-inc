Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-agreement-payment-request-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-agreement-payment-request-workflow marketplace-agreement-billing-adjustment-workflow marketplace-agreement-validation-error-paths
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|SendAgreementPaymentRequest|4xx->4xx|$.__type
Issue-Key: marketplace-agreement|SendAgreementPaymentRequest|4xx->4xx|$.fields
Issue-Key: marketplace-agreement|SendAgreementPaymentRequest|4xx->4xx|$.message
Issue-Key: marketplace-agreement|SendAgreementPaymentRequest|4xx->4xx|$.reason
Issue-Key: marketplace-agreement|SendAgreementPaymentRequest|4xx->4xx|$.requestId
Issue-Key: marketplace-agreement|GetBillingAdjustmentRequest|4xx->4xx|$.message
Issue-Key: marketplace-agreement|GetBillingAdjustmentRequest|4xx->4xx|$.resourceId
Issue-Key: marketplace-agreement|GetBillingAdjustmentRequest|4xx->4xx|$.resourceType
Issue-Key: marketplace-agreement|ListAgreementCancellationRequests|4xx->4xx|$.fields[name="partyType"].message
Issue-Key: marketplace-agreement|ListAgreementCancellationRequests|4xx->4xx|$.requestId
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# marketplace-agreement error responses misclassify input faults and omit requestId

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendAgreementPaymentRequest

## Description
Three distinct error-modelling defects in the marketplace-agreement (AWSMPCommerceService_v20200301, awsJson1_0) emulator. All three are about the error SHAPE the client receives, not about generated ids or message wording. THIS ISSUE BUNDLES 3 CAUSES — all three must be fixed before it is closed.

Cause 1 of 3: SendAgreementPaymentRequest classifies an unusable agreementId as ResourceNotFoundException instead of ValidationException.
AWS: Smithy: SendAgreementPaymentRequest.agreementId -> AgreementId @required @length(1,64) @pattern ^[A-Za-z0-9_/-]+$; the operation models both ValidationException (client, httpError 400) and ResourceNotFoundException (client, httpError 404). ValidationException carries members reason -> ValidationExceptionReason (which includes the value INVALID_AGREEMENT_ID) and fields -> ValidationExceptionFieldList of ValidationExceptionField{name,message}. The recorded AWS response is HTTP 400 with __type com.amazonaws.mp.commerce.v20200301#ValidationException, reason INVALID_AGREEMENT_ID and a fields entry naming member agreementId (recorded message, as an example: "Member is invalid"; top-level message example: "Provided agreement id is invalid."). So AWS rejects an agreementId it cannot accept as an INPUT fault attributed to the member, not as a missing resource.
Emulator: returns __type ResourceNotFoundException with no reason and no fields (message example: "Provided agreement does not exist.").
Why: the error code is the contract a client branches on. An SDK caller catching ValidationException to surface "fix this field" sees ResourceNotFoundException instead and takes the retry/create path for a request that can never succeed; the per-field diagnostics (reason, fields[].name) that tell it WHICH member is at fault are absent entirely. The two errors also carry different modelled httpError (400 vs 404), so error-class-based handling is wrong in both directions.
Fix: the SendAgreementPaymentRequest handler's input validation must run the agreementId member check before resource lookup and raise ValidationException with reason INVALID_AGREEMENT_ID plus a ValidationExceptionField for agreementId, rather than falling through to the not-found path in state lookup.
Traces: marketplace-agreement-agreement-payment-request-workflow#task#3.

Cause 2 of 3: GetBillingAdjustmentRequest attributes a missing agreement to the wrong resource.
AWS: Smithy: GetBillingAdjustmentRequest input has agreementId (@required, AgreementId) and billingAdjustmentRequestId (@required, @pattern ^ba-[a-zA-Z0-9]+$); the only modelled resource error is ResourceNotFoundException{requestId,message,resourceId,resourceType}, where resourceType is the enum ResourceType with values including both Agreement and BillingAdjustmentRequest. The recording shows AWS resolving the parent agreement FIRST: the response names resourceType "Agreement" with resourceId set to the agreementId from the request (message example: "Provided agreement does not exist.").
Emulator: returns resourceType "BillingAdjustmentRequest" with resourceId set to the billingAdjustmentRequestId from the request (message example: "Provided billing adjustment request does not exist."), i.e. it reports the child as missing when it is the parent agreement that does not exist.
Why: resourceType/resourceId are the machine-readable part of this error and are how a client decides what to repair. A caller told the billing adjustment request is missing will retry with a different adjustment id against an agreement that does not exist, looping instead of correcting the agreementId.
Fix: the GetBillingAdjustmentRequest handler must resolve and existence-check the agreement before the billing adjustment request, and populate ResourceNotFoundException.resourceType/resourceId from whichever resource actually failed to resolve (Agreement + the request's agreementId in this case).
Traces: marketplace-agreement-billing-adjustment-workflow#task#2.

Cause 3 of 3: ValidationException is serialized without the modelled requestId member.
AWS: Smithy: ValidationException members are requestId -> RequestId @length(1,128) @pattern ^[A-Za-z0-9-]+$ ("The unique identifier associated with the error"), message, reason, fields. Both recorded ValidationException bodies populate requestId (recorded example values 5abb69a3-282c-4619-a443-b3aead6d1abb and fc89b19d-2d8a-46ce-a4c5-a7ba6f49321c — the VALUE is per-request and is not the requirement; its PRESENCE is).
Emulator: omits requestId from the ValidationException body entirely (got None) on every validation path observed, both on SendAgreementPaymentRequest and on ListAgreementCancellationRequests.
Why: a client that logs or reports the error's requestId for support correlation gets null, and any deserializer or assertion that treats the modelled identifier as always-present on this error sees an incomplete error object.
Fix: the service's error serializer for the marketplace-agreement exception shapes must emit a requestId matching ^[A-Za-z0-9-]+$ (1..128 chars) for every ValidationException it renders, sourced from the per-request id the handler already has.
Traces: marketplace-agreement-validation-error-paths#task#2 (ListAgreementCancellationRequests), also visible on marketplace-agreement-agreement-payment-request-workflow#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (11 fixtures, 35 steps, 18 passed, score 51.4%, bootstrap_signal false, state_reset_failures 0).

Cause 1 — fixture marketplace-agreement-agreement-payment-request-workflow step task#3, SendAgreementPaymentRequest, expected HTTP 400 / actual HTTP 400:
  $.__type: expected 'com.amazonaws.mp.commerce.v20200301#ValidationException', got 'ResourceNotFoundException'
  $.reason: expected 'INVALID_AGREEMENT_ID', got None (member absent)
  $.fields: expected [{'name': 'agreementId', 'message': 'Member is invalid'}], got None (member absent)
  $.requestId: expected present (example '5abb69a3-282c-4619-a443-b3aead6d1abb'), got None
  $.message: expected 'Provided agreement id is invalid.' (example), got 'Provided agreement does not exist.'

Cause 2 — fixture marketplace-agreement-billing-adjustment-workflow step task#2, GetBillingAdjustmentRequest, expected HTTP 400 / actual HTTP 400:
  $.resourceType: expected 'Agreement', got 'BillingAdjustmentRequest'
  $.resourceId: expected the request's agreementId (example 'sim2real-probe-marketplace-agreement-agmt-95636502'), got the request's billingAdjustmentRequestId (example 'ba-sim2realprobemarketplaceagreementfakedcef07e6')
  $.message: expected 'Provided agreement does not exist.' (example), got 'Provided billing adjustment request does not exist.'

Cause 3 — fixture marketplace-agreement-validation-error-paths step task#2, ListAgreementCancellationRequests, expected HTTP 400 / actual HTTP 400:
  $.requestId: expected present (example 'fc89b19d-2d8a-46ce-a4c5-a7ba6f49321c'), got None
  (the same step's $.fields[name="partyType"].message differs only by a trailing period — 'Member is invalid' vs 'Member is invalid.' — which is message text, not part of this finding)

No open issue references these fixtures or steps; list_existing_issues found no marketplace-agreement issues at all.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus), or replay the individual fixtures below.
3. Cause 1: fixture marketplace-agreement-agreement-payment-request-workflow, step task#3 — call SendAgreementPaymentRequest with an agreementId the service cannot accept; observe __type ResourceNotFoundException with no reason/fields instead of ValidationException with reason INVALID_AGREEMENT_ID and a fields entry for agreementId.
4. Cause 2: fixture marketplace-agreement-billing-adjustment-workflow, step task#2 — call GetBillingAdjustmentRequest with an agreementId that does not exist plus any billingAdjustmentRequestId; observe ResourceNotFoundException resourceType 'BillingAdjustmentRequest' / resourceId = the ba- id instead of resourceType 'Agreement' / resourceId = the agreementId.
5. Cause 3: fixture marketplace-agreement-validation-error-paths, step task#2 — call ListAgreementCancellationRequests with an invalid partyType; observe the 400 ValidationException body has no requestId member at all.
