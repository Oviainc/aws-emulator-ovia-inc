Issue-Source: trace-sweep-divergence
Issue-Trace: networkmanager-error-nonexistent-parent-references
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkmanager-error-nonexistent-parent-references
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager|AssociateLink|4xx->4xx|$.Fields
Issue-Key: networkmanager|AssociateLink|4xx->4xx|$.Message
Issue-Key: networkmanager|AssociateLink|4xx->4xx|$.Reason
Issue-Key: networkmanager|AssociateLink|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkmanager|AssociateLink|4xx->4xx|status
Issue-Commit: d6aa252f09dfe86e7f4b30505c74249c49cdaee6

# AssociateLink reports a nonexistent globalNetworkId as 404 ResourceNotFoundException instead of the field-scoped 400 ValidationException AWS returns

> _Note: this issue was identified against the code base at commit `d6aa252f09dfe86e7f4b30505c74249c49cdaee6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateLink

## Description
One root cause in the networkmanager crate's AssociateLink handler: the not-found refusal for the request's parent global network is raised under the wrong modeled error shape, with the wrong HTTP status, the wrong error code, the wrong message, and without the field-level detail AWS attaches.

=== Cause 1 of 1: a nonexistent GlobalNetworkId on AssociateLink is answered as ResourceNotFoundException/404, not as a FieldValidationFailed ValidationException/400 ===
AWS: AssociateLink is POST /global-networks/{GlobalNetworkId}/link-associations, with GlobalNetworkId bound as an @httpLabel and required (Smithy: AssociateLink, input members GlobalNetworkId/DeviceId/LinkId, all required, length 0..50). The operation models BOTH ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). Recorded behavior: when the global network named in the URI path does not exist, the service does NOT take the 404 path — it refuses the request as a path-parameter validation failure: HTTP status 400, header x-amzn-errortype: ValidationException, body Reason: "FieldValidationFailed", Message: "Global network not found.", and Fields: [{ "Name": "globalNetworkId", "Message": "Global network not found." }]. In that payload the fixed text is the sentence "Global network not found." (used both as the top-level Message and as the field's Message) and the Reason enum value FieldValidationFailed (Smithy: ValidationExceptionReason = UnknownOperation|CannotParse|FieldValidationFailed|Other); the only variable part is Fields[].Name, which is the wire/path name of the offending input member — here the lower-camel "globalNetworkId" for the GlobalNetworkId label, not the Smithy member spelling and not the id value. Smithy backs this shape: ValidationException carries required Message plus optional Reason and Fields (ValidationExceptionField = required Name + required Message).
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and Message "Resource not found.". Reason is absent, Fields is absent, and the message is a generic constant that names neither the resource kind nor the offending input member. (ResourceNotFoundException is also modeled with REQUIRED ResourceId and ResourceType members, so the emulator's chosen shape is additionally under-populated for the error it claims to be raising.)
Why: a client that handles a bad path parameter by catching ValidationException and reading Reason/Fields to learn which input member is wrong sees an uncaught ResourceNotFoundException instead, and gets no field name at all — so form/CLI validation and retry-vs-abort branching keyed on the modeled 400 never fires, while code keyed on 404 "resource missing" fires where AWS would not. The generic "Resource not found." also hides which of the three identifiers (globalNetworkId vs deviceId vs linkId) the caller got wrong, which is exactly the information AWS puts in Fields.
Fix: the parent-reference lookup in the AssociateLink handler's input validation (the networkmanager crate's AssociateLink operation handler / its shared global-network resolution helper). The global-network-does-not-exist branch must build the modeled ValidationException with Reason=FieldValidationFailed and a Fields entry whose Name is the offending member's wire name, rather than falling through to the generic ResourceNotFoundException the crate uses for missing resources. Note: open issue 20261028T052913Z-networkmanager-associatecustomergateway-wrong-behavior concerns a 404 on the sibling AssociateCustomerGateway for an EMPTY LinkId; it does not cover this operation's nonexistent-global-network refusal being the wrong error class and missing Reason/Fields, so it does not cover this cause.
Traces: networkmanager-error-nonexistent-parent-references#task#4

## Evidence
Corpus: simparity-wire-aws-v1 — 82 steps, 80 passed, 2 mismatched (score 97.6%). One divergence cluster.

Fixture networkmanager-error-nonexistent-parent-references, step task#4, operation AssociateLink, verdict mismatch (cluster divergence_key service=networkmanager|AssociateLink|status=400->404|keys=$.Fields,$.Message,$.Reason,$headers.x-amzn-errortype,status; 1 affected step: networkmanager-error-nonexistent-parent-references#task#4):
  status: expected 400, got 404
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'
  $.Message: expected 'Global network not found.', got 'Resource not found.'
  $.Reason: expected 'FieldValidationFailed', got None (member absent)
  $.Fields: expected [{'Name': 'globalNetworkId', 'Message': 'Global network not found.'}], got None (member absent)

Smithy (services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json):
  AssociateLink: POST /global-networks/{GlobalNetworkId}/link-associations, success code 200; GlobalNetworkId @httpLabel @required (length 0..50); DeviceId and LinkId @required; errors include ValidationException(400) and ResourceNotFoundException(404).
  ValidationException: client error, httpError 400; members Message (required), Reason (ValidationExceptionReason), Fields (ValidationExceptionFieldList).
  ValidationExceptionReason enum values: UnknownOperation, CannotParse, FieldValidationFailed, Other.
  ValidationExceptionField: required Name + required Message.
  ResourceNotFoundException: client error, httpError 404; members Message, ResourceId, ResourceType all required (the emulator's response populates only Message).

## Steps to Reproduce
1. Boot the emulator and replay fixture networkmanager-error-nonexistent-parent-references; the divergence is step task#4.
2. Equivalent by hand: without creating any global network (or using an id that was never minted), call AssociateLink against a nonexistent global network — POST /global-networks/<nonexistent-global-network-id>/link-associations with a body naming any DeviceId and LinkId.
3. Expected (AWS, per the recording): HTTP 400, header x-amzn-errortype: ValidationException, body {"Message": "Global network not found.", "Reason": "FieldValidationFailed", "Fields": [{"Name": "globalNetworkId", "Message": "Global network not found."}]}.
4. Observed: HTTP 404, header x-amzn-errortype: ResourceNotFoundException, body {"Message": "Resource not found."} — no Reason, no Fields.
5. Control: the same fixture's other steps against existing parents pass, so the divergence is specific to the nonexistent-parent refusal path.
