Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-procurement-portal-preference-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-procurement-portal-preference-lifecycle
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|SendProcurementPortalValidation|4xx->2xx|$.Message
Issue-Key: invoicing|SendProcurementPortalValidation|4xx->2xx|$.__type
Issue-Key: invoicing|SendProcurementPortalValidation|4xx->2xx|status
Issue-Key: invoicing|UpdateProcurementPortalPreferenceStatus|4xx->2xx|$.Message
Issue-Key: invoicing|UpdateProcurementPortalPreferenceStatus|4xx->2xx|$.__type
Issue-Key: invoicing|UpdateProcurementPortalPreferenceStatus|4xx->2xx|status
Issue-Key: invoicing|GetProcurementPortalPreference|2xx->2xx|$.ProcurementPortalPreference.EinvoiceDeliveryPreferenceStatus
Issue-Key: invoicing|GetProcurementPortalPreference|2xx->2xx|$.ProcurementPortalPreference.Version
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# invoicing procurement-portal preference status machine: two refusals missing and the read-back serves the rejected status

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendProcurementPortalValidation

## Description
Three net-new root causes on the invoicing procurement-portal preference lifecycle. The emulator has no refusal path for the two status-machine gates AWS enforces on a preference that is still PENDING_VERIFICATION, so both mutations are accepted with HTTP 200 and the preference's subsequent read-back reports state AWS never puts it in. A fixer must close all THREE causes below before submitting.

Cause 1 of 3 - SendProcurementPortalValidation does not refuse an unconfirmed test-e-invoice delivery (fixture invoicing-procurement-portal-preference-lifecycle, step task#6).
AWS: recorded behavior only - the Smithy model (SendProcurementPortalValidation, errors AccessDeniedException/InternalServerException/ResourceNotFoundException/ThrottlingException/ValidationException; doc only says the operation 'initiates the validation process by issuing a validation code that confirms ownership and connectivity of the configured procurement portal endpoint') documents no delivery-confirmation gate, so the requirement is taken from the recorded response. When the service cannot confirm delivery of the test e-invoice to the configured portal endpoint, AWS answers HTTP 400 with __type 'ValidationException' and the Message member set to the fixed, entirely request-independent text: "We could not confirm delivery of the test e-invoice. The OTP may have been sent but rejected by your procurement portal's business-rule validation. Check your portal's rejected-invoice log for the OTP (for example, in Coupa open Setup then Integrations then cXML Errors; in SAP Business Network review your Invoice Rules) and submit it to complete verification. If the OTP is not there, verify your portal configuration or contact support." Every word of that message is fixed text for this error class - it interpolates nothing from the request or from state.
Emulator: answers HTTP 200 with a success body; no __type and no Message member are present at all, so the refusal does not exist in any form.
Why: a client driving the documented send/verify handshake is told the validation code was delivered when it was not. It then polls VerifyProcurementPortalValidation with a code that can never arrive and has no way to discover that it must look in the portal's rejected-invoice log, so the verification flow dead-ends with no diagnosable error.
Fix: the SendProcurementPortalValidation handler and its refusal path - the delivery-confirmation gate must run before the success response is built, and must emit the ValidationException above through the service's JSON error serializer (__type + Message).

Cause 2 of 3 - UpdateProcurementPortalPreferenceStatus accepts a PENDING_VERIFICATION -> SUSPENDED transition that AWS reserves to itself (fixture invoicing-procurement-portal-preference-lifecycle, step task#8).
AWS: recorded behavior only - the model defines EinvoiceDeliveryPreferenceStatus over ProcurementPortalPreferenceStatus (PENDING_VERIFICATION, VALIDATED, TEST_INITIALIZED, TEST_INITIALIZATION_FAILED, TEST_FAILED, ACTIVE, SUSPENDED) and lists ValidationException (400) among the operation's errors, but states no transition table, so the rule is taken from the recorded response. A caller-requested transition out of PENDING_VERIFICATION into SUSPENDED is refused HTTP 400 with __type 'ValidationException' and a Message of the form: the fixed prefix "Given detail ", then the request detail echoed as a JSON object, then the fixed infix " in request is invalid because [", then the violated rules joined with ", ", then the fixed suffix "]". The rule sentences are themselves templates with fixed text and substituted parts: "Transition of <StatusMemberName> from <CurrentStatus> to <RequestedStatus> can only be done by AWS" and "Transition of <StatusMemberName> from <CurrentStatus> to <RequestedStatus> is not allowed". The substituted parts are the status member being written (EinvoiceDeliveryPreferenceStatus), the status currently held in state (PENDING_VERIFICATION in the recording) and the status asked for in the request (SUSPENDED in the recording); the echoed detail object carries the caller's awsAccountId, procurementPortalPreferenceArn, the requested einvoiceDeliveryPreferenceStatus and the caller's einvoiceDeliveryPreferenceStatusReason. The recorded instance of that template is: Given detail {"awsAccountId":"000000000000","procurementPortalPreferenceArn":"arn:aws:invoicing::000000000000:procurement-portal-preference/000000000001","einvoiceDeliveryPreferenceStatus":"SUSPENDED","einvoiceDeliveryPreferenceStatusReason":"probe-suspend"} in request is invalid because [Transition of EinvoiceDeliveryPreferenceStatus from PENDING_VERIFICATION to SUSPENDED can only be done by AWS, Transition of EinvoiceDeliveryPreferenceStatus from PENDING_VERIFICATION to SUSPENDED is not allowed] - the account id, ARN and reason string there are that run's values filling the template, not part of it.
Emulator: answers HTTP 200 and writes the requested SUSPENDED status onto the preference; no __type and no Message member are emitted.
Why: a client can drive a preference into a state the service reserves to AWS, and its own state machine then disagrees with the service's. Code that treats SUSPENDED as an AWS-initiated signal (to stop sending invoices, to alert an operator) fires on a state the real service would have refused to enter, and a client that expects the 400 to tell it which transitions are caller-initiated gets no diagnosis at all.
Fix: the UpdateProcurementPortalPreferenceStatus handler's validation - a transition table over ProcurementPortalPreferenceStatus consulted before the status write, raising the ValidationException above (with the detail echo and the joined rule list) for a caller-requested transition AWS does not allow.

Cause 3 of 3 - GetProcurementPortalPreference serves the rejected status and a Version behind the accepted-write count (fixture invoicing-procurement-portal-preference-lifecycle, step task#9).
AWS: GetProcurementPortalPreference returns the required ProcurementPortalPreference structure, whose EinvoiceDeliveryPreferenceStatus reflects only transitions the service accepted and whose required Version (Long) counts the writes the service actually applied. After the two refusals above, the preference is still PENDING_VERIFICATION and its Version has advanced only for the accepted writes in the lifecycle (Version 4 in the recording).
Emulator: returns EinvoiceDeliveryPreferenceStatus 'SUSPENDED' and Version 3 where the recording has 'PENDING_VERIFICATION' and 4 - i.e. the read-back exposes the state written by the transition AWS refuses, and its write counter is one short of the accepted-write sequence.
Why: the read-back is how a client confirms a preference's lifecycle position and how an optimistic-concurrency caller picks a Version. Serving a status the service never enters makes the confirmation step report a different lifecycle than AWS, and an off-by-one Version breaks any caller that compares the version it wrote against the version it reads.
Fix: the procurement-portal preference state in the invoicing crate, plus the Version bookkeeping shared by its mutating handlers - status must change only on an accepted transition, and Version must advance once per applied write (and not at all on a refusal).

Related open issues that do NOT cover these (no replay key for these steps): 20261028T035328Z-invoicing-updateprocurementportalpreferencestatus-wrong-behavior.md (status written onto a disabled plane) and 20261028T151600Z-invoicing-sendprocurementportalvalidation-wrong-behavior--r3.md (the issued code's clock mixing).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 89.1%, 4 divergence clusters, all invoicing).

Cause 1 - fixture invoicing-procurement-portal-preference-lifecycle step task#6, SendProcurementPortalValidation, verdict mismatch: expected status 400, got 200; $.__type missing (expected 'ValidationException', got None); $.Message missing (expected "We could not confirm delivery of the test e-invoice. The OTP may have been sent but rejected by your procurement portal's business-rule validation. Check your portal's rejected-invoice log for the OTP (for example, in Coupa open Setup then Integrations then cXML Errors; in SAP Business Network review your Invoice Rules) and submit it to complete verification. If the OTP is not there, verify your portal configuration or contact support.", got None).

Cause 2 - fixture invoicing-procurement-portal-preference-lifecycle step task#8, UpdateProcurementPortalPreferenceStatus, verdict mismatch: expected status 400, got 200; $.__type missing (expected 'ValidationException'); $.Message missing (expected 'Given detail {"awsAccountId":"000000000000","procurementPortalPreferenceArn":"arn:aws:invoicing::000000000000:procurement-portal-preference/000000000001","einvoiceDeliveryPreferenceStatus":"SUSPENDED","einvoiceDeliveryPreferenceStatusReason":"probe-suspend"} in request is invalid because [Transition of EinvoiceDeliveryPreferenceStatus from PENDING_VERIFICATION to SUSPENDED can only be done by AWS, Transition of EinvoiceDeliveryPreferenceStatus from PENDING_VERIFICATION to SUSPENDED is not allowed]').

Cause 3 - fixture invoicing-procurement-portal-preference-lifecycle step task#9, GetProcurementPortalPreference, verdict mismatch, status 200 both sides: $.ProcurementPortalPreference.EinvoiceDeliveryPreferenceStatus value_mismatch (expected 'PENDING_VERIFICATION', got 'SUSPENDED'); $.ProcurementPortalPreference.Version value_mismatch (expected 4, got 3).

Model grounding (services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json): SendProcurementPortalValidation and UpdateProcurementPortalPreferenceStatus both list ValidationException with httpError 400; ProcurementPortalPreferenceStatus enum = [PENDING_VERIFICATION, VALIDATED, TEST_INITIALIZED, TEST_INITIALIZATION_FAILED, TEST_FAILED, ACTIVE, SUSPENDED]; ProcurementPortalPreference.Version is a required Long; neither operation's documentation states the transition table or the delivery-confirmation gate, so those two messages are grounded in the recording.

## Steps to Reproduce
1. Replay fixture invoicing-procurement-portal-preference-lifecycle against the emulator (replay_wire_corpus on trace set simparity-wire-aws-v1).
2. Step task#6: call SendProcurementPortalValidation for the preference whose test e-invoice delivery cannot be confirmed - the emulator answers 200 instead of 400 ValidationException with the delivery-confirmation Message.
3. Step task#8: call UpdateProcurementPortalPreferenceStatus with EinvoiceDeliveryPreferenceStatus=SUSPENDED while the preference is PENDING_VERIFICATION - the emulator answers 200 and applies the status instead of 400 ValidationException naming the disallowed/AWS-only transition.
4. Step task#9: call GetProcurementPortalPreference for the same ARN - the emulator returns EinvoiceDeliveryPreferenceStatus SUSPENDED and Version 3 where the recording has PENDING_VERIFICATION and Version 4.
