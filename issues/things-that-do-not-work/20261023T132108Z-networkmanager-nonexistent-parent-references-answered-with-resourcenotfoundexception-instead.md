Issue-Source: trace-sweep-divergence
Issue-Trace: networkmanager-error-nonexistent-parent-references
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkmanager-error-nonexistent-parent-references
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager|CreateSite|4xx->4xx|$.Fields
Issue-Key: networkmanager|CreateSite|4xx->4xx|$.Message
Issue-Key: networkmanager|CreateSite|4xx->4xx|$.Reason
Issue-Key: networkmanager|CreateSite|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkmanager|CreateSite|4xx->4xx|status
Issue-Key: networkmanager|CreateLink|4xx->4xx|$.Fields
Issue-Key: networkmanager|CreateLink|4xx->4xx|$.Message
Issue-Key: networkmanager|CreateLink|4xx->4xx|$.Reason
Issue-Key: networkmanager|CreateLink|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkmanager|CreateLink|4xx->4xx|status
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# networkmanager nonexistent parent references answered with ResourceNotFoundException instead of ValidationException

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSite

## Description
When a create call references a parent resource that does not exist, networkmanager classifies the failure as a field-validation failure (400 ValidationException carrying Reason and Fields), not as a missing resource. The emulator raises ResourceNotFoundException (404) instead, so the error class, the HTTP status and the structured Fields/Reason diagnostics are all wrong. This bundle carries TWO distinct causes; both must be closed.

Cause 1 of 2 — CreateSite with an unknown GlobalNetworkId.
AWS: Smithy: CreateSite -> POST /global-networks/{GlobalNetworkId}/sites models both ValidationException (@httpError 400) and ResourceNotFoundException (@httpError 404); ValidationException members are Message (required), Reason (ValidationExceptionReason enum [UnknownOperation, CannotParse, FieldValidationFailed, Other]) and Fields (list of ValidationExceptionField{Name, Message}). The recording shows the unknown-parent case answered as 400 with x-amzn-errortype ValidationException, Reason 'FieldValidationFailed' and Fields naming the offending input member: [{'Name': 'globalNetworkId', 'Message': 'Global network not found.'}] (the message text is an example; the requirement is the 400/ValidationException classification with Reason=FieldValidationFailed and a Fields entry whose Name is the rejected member, globalNetworkId).
Emulator: 404 with x-amzn-errortype ResourceNotFoundException and a body carrying only a generic Message ('Resource not found.'); $.Reason and $.Fields are absent entirely.
Why: SDK clients branch on the modeled error shape. A caller catching ValidationException to report which input was bad instead sees an unmodeled-for-this-path 404, loses the Fields[].Name that tells it the globalNetworkId was at fault, and retry/error-mapping logic keyed on 400-vs-404 takes the wrong branch.
Fix: the CreateSite handler's input validation in the networkmanager crate (src/ops/) must raise ValidationException with Reason=FieldValidationFailed and a Fields entry for globalNetworkId when the parent lookup misses, rather than surfacing the store's not-found as ResourceNotFoundException.
Traces: networkmanager-error-nonexistent-parent-references#task#1.

Cause 2 of 2 — CreateLink with an unknown SiteId.
AWS: Smithy: CreateLink -> POST /global-networks/{GlobalNetworkId}/links with required SiteId; same modeled error set and same ValidationException shape. Recording: 400, x-amzn-errortype ValidationException, Reason 'FieldValidationFailed', Fields [{'Name': 'siteId', 'Message': 'Site not found.'}] (message text an example; the requirement is the 400/ValidationException classification with Reason=FieldValidationFailed and a Fields entry naming siteId).
Emulator: 404 with x-amzn-errortype ResourceNotFoundException and a generic Message, no Reason and no Fields.
Why: identical to cause 1, for the link's site reference: the client cannot tell which member it got wrong and sees the wrong error class and status.
Fix: the CreateLink handler's SiteId validation in the networkmanager crate (src/ops/).
Traces: networkmanager-error-nonexistent-parent-references#task#3.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

Cause 1 CreateSite (fixture networkmanager-error-nonexistent-parent-references, step task#1) — 'status: expected 400, got 404'; '$headers.x-amzn-errortype: value_mismatch (expected 'ValidationException', got 'ResourceNotFoundException')'; '$.Reason: missing (expected 'FieldValidationFailed', got None)'; '$.Fields: missing (expected [{'Message': 'Global network not found.', 'Name': 'globalNetworkId'}], got None)'; '$.Message: value_mismatch (expected 'Global network not found.', got 'Resource not found.')'.

Cause 2 CreateLink (same fixture, step task#3) — 'status: expected 400, got 404'; '$headers.x-amzn-errortype: value_mismatch (expected 'ValidationException', got 'ResourceNotFoundException')'; '$.Reason: missing (expected 'FieldValidationFailed', got None)'; '$.Fields: missing (expected [{'Message': 'Site not found.', 'Name': 'siteId'}], got None)'; '$.Message: value_mismatch (expected 'Site not found.', got 'Resource not found.')'.

Model basis: services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json — CreateSite and CreateLink both bind ValidationException (@httpError 400); ValidationException{Message required, Reason: ValidationExceptionReason enum including FieldValidationFailed, Fields: list<ValidationExceptionField{Name, Message}>}.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture networkmanager-error-nonexistent-parent-references, step task#1 — CreateSite under a global network id that was never created -> emulator answers 404 ResourceNotFoundException instead of 400 ValidationException with Reason=FieldValidationFailed and Fields[0].Name=globalNetworkId.
4. Cause 2: same fixture, step task#3 — CreateLink with a SiteId that was never created -> emulator answers 404 ResourceNotFoundException instead of 400 ValidationException with Reason=FieldValidationFailed and Fields[0].Name=siteId.
