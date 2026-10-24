Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-batch-get-invoice-profile
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-batch-get-invoice-profile
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|$.__type
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|$.message
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|status
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# BatchGetInvoiceProfile returns 200 instead of the modeled 403 AccessDeniedException for unauthorized account ids

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetInvoiceProfile

## Description
Cause 1 of 1: BatchGetInvoiceProfile performs no authorization check on the requested AccountIds, so a request the real service refuses is answered as a success.

AWS: BatchGetInvoiceProfile (Smithy: services/service-com-amazonaws-invoicing-invoicing — input BatchGetInvoiceProfileRequest.AccountIds, required, @length min 1 max 1000; modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400)). AccessDeniedException is documented "You don't have sufficient access to perform this action." and carries httpError 403 with members message (BasicString, length 0..1024) and resourceName. The recorded response for this step is HTTP 403 with a JSON body whose __type is 'AccessDeniedException' and a message explaining that the calling account is not authorized to access resources for the requested account (recorded example text: 'Account 000000000000 is not authorized to access resources for account 000000000000'). The requirement here is the 403 status and the AccessDeniedException error code/__type; the exact message text is an example only, since the model does not state its wording.

Emulator: the same request returns HTTP 200 with a success-shaped BatchGetInvoiceProfileResponse body. Neither $.__type nor $.message is present at all (replay detail: "status: expected 403, got 200; $.__type: missing (expected 'AccessDeniedException', got None); $.message: missing"). So the operation never evaluates whether the caller may read the requested account ids and unconditionally returns a profile list.

Why: a client that relies on AWS's behavior sees an authorization failure as a successful read. SDK error handling for AccessDeniedException (retry/escalate/surface a permissions problem to the operator) never runs; instead the caller consumes a 200 whose Profiles list silently omits or fabricates data for accounts it is not entitled to, and any code that distinguishes "no such profile" (404) from "not permitted" (403) from "empty result" (200) takes the wrong branch. For a billing-data API this turns a deny into an apparent read of another account's invoice profile.

Fix: belongs in the invoicing BatchGetInvoiceProfile handler's request-authorization path (the check that runs before the profile lookup, and the error serializer that must render AccessDeniedException as HTTP 403 with __type 'AccessDeniedException' and a message member). The handler must reject account ids the caller is not authorized for with the modeled 403 AccessDeniedException rather than falling through to the success response.

Traces: fixture invoicing-batch-get-invoice-profile, step task#1 (cluster of 1; affected: invoicing-batch-get-invoice-profile#task#1).

Note: none of the currently open invoicing issues cover this operation — they concern CreateProcurementPortalPreference tag quota, SendProcurementPortalValidation / VerifyProcurementPortalValidation version advancement, and the GetInvoicePDF / ListProcurementPortals / ListProcurementPortalSuppliers / UpdateProcurementPortalPreferenceStatus stubs.

## Evidence
Replay step invoicing-batch-get-invoice-profile#task#1, verdict=mismatch, divergence_key service=invoicing|BatchGetInvoiceProfile|status=403->200|keys=$.__type,$.message,status.
Expected (recorded AWS response): HTTP 403, body $.__type = 'AccessDeniedException', $.message = 'Account 000000000000 is not authorized to access resources for account 000000000000' (message text is an example; status + error code are the requirement).
Actual (emulator): HTTP 200, success body; $.__type missing, $.message missing.
Model support: BatchGetInvoiceProfile declares AccessDeniedException with httpError 403 (awsQueryError code InvoicingAccessDenied), so a 403 AccessDeniedException is a modeled outcome of this operation.

## Steps to Reproduce
1. Boot the emulator built from this checkout (data listener answered on :34299, /__admin/health 200).
2. Replay wire fixture invoicing-batch-get-invoice-profile, step task#1 — a BatchGetInvoiceProfile call whose AccountIds the caller is not authorized to read.
3. Observe the emulator answers HTTP 200 with a success BatchGetInvoiceProfileResponse body instead of the recorded HTTP 403 with __type 'AccessDeniedException' and a message member.
