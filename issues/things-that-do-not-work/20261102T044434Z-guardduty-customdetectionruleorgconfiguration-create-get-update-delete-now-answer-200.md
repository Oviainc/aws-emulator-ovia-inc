Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-custom-detection-rule-org-configuration-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-custom-detection-rule-org-configuration-errors
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->2xx|$
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->2xx|status
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->2xx|$.__type
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->2xx|$.message
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->2xx|$.type
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->2xx|status
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->2xx|$
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->2xx|status
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->2xx|$
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->2xx|status
Issue-Commit: 9fcc5571ff6cbb375c3cc1ebb135d0cedced4f12

# CustomDetectionRuleOrgConfiguration Create/Get/Update/Delete now answer 200 success to a caller that is not the delegated administrator

> _Note: this issue was identified against the code base at commit `9fcc5571ff6cbb375c3cc1ebb135d0cedced4f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomDetectionRuleOrgConfiguration

## Description
The four single-rule CustomDetectionRuleOrgConfiguration operations (Create, Get, Update, Delete) apply no authorization gate at all and now answer a caller that is not the organization's delegated administrator with a SUCCESS instead of a refusal. 4 causes.

Shared AWS grounding for all four causes: the *CustomDetectionRuleOrgConfiguration operations are restricted to the organization's delegated administrator for GuardDuty. A caller without that standing is refused before the ruleId or the configuration is resolved. Recorded refusal shape (recorded behavior only for the status/code pairing, which is unusual and is taken verbatim from the recorded responses, not inferred): HTTP 400, response header x-amzn-errortype: 'AccessDeniedException', body {"message": "The request is rejected because the caller is not authorized to call this API.", "__type": "InvalidInputException", "type": "InvalidInputException"}. The message is entirely FIXED text — it has no variable parts, so nothing is interpolated from the request or from state. Smithy (services/service-com-amazonaws-guardduty-guarddutyapiservice): each of these four operations models AccessDeniedException (httpError 403), BadRequestException (400) and ResourceNotFoundException (404), so the authorization outcome is a modeled error on every one of them; the recorded wire form for this refusal is the 400 + x-amzn-errortype AccessDeniedException pairing above.

Cause 1 of 4: CreateCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the refusal above — HTTP 400, x-amzn-errortype 'AccessDeniedException', __type/type 'InvalidInputException', the fixed authorization sentence — emitted before the ruleId is resolved.
Emulator: HTTP 200 with an empty body ({}), i.e. the organization configuration is created for an unauthorized caller. No message, no __type, no type and no x-amzn-errortype header.
Why: a client relying on AWS's behavior believes the delegated-admin restriction is in force. An unauthorized account silently mutates organization-wide detection-rule configuration, and a test asserting that the restriction refuses the call passes against the emulator while the restriction does not exist.
Fix: an authorization check at the head of the CreateCustomDetectionRuleOrgConfiguration handler, ahead of rule resolution, emitting the modeled refusal.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#3.

Cause 2 of 4: GetCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the same 400 / x-amzn-errortype AccessDeniedException / InvalidInputException refusal with the same fixed sentence.
Emulator: HTTP 200 with its normal success projection — $.message, $.__type, $.type and the x-amzn-errortype header are all absent.
Why: an unauthorized caller is handed organization configuration data (the rule's mode and account include/exclude lists) instead of being refused — a read-side authorization bypass, not merely a mis-worded error.
Fix: an authorization check at the head of the GetCustomDetectionRuleOrgConfiguration handler, ahead of configuration lookup.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#4.

Cause 3 of 4: UpdateCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the same refusal.
Emulator: HTTP 200 with an empty body ({}) and none of the error fields or the x-amzn-errortype header.
Why: as cause 1, on a mutating path — an unauthorized account can re-scope which member accounts an organization's detection rule applies to, and the caller gets no signal that it was never entitled to.
Fix: an authorization check at the head of the UpdateCustomDetectionRuleOrgConfiguration handler.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#5.

Cause 4 of 4: DeleteCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the same refusal.
Emulator: HTTP 200 with an empty body ({}) and none of the error fields or the x-amzn-errortype header.
Why: as cause 3 — an unauthorized account can tear down an organization's detection-rule configuration and the call reports success.
Fix: an authorization check at the head of the DeleteCustomDetectionRuleOrgConfiguration handler.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#6.

Related open issue, which does NOT cover these four causes (the replay reports no owner for these steps, because that issue records the refusal-vs-404 form these operations used to take, while they now answer 200 success): 20261029T190006Z-guardduty-custom-detection-rule-org-configuration-ignores-the-delegated.md. Also related and likewise not covering them: 20261029T183757Z-guardduty-performs-no-authorization-identity-policy-deny-is.md.

## Evidence
Cause 1 (CreateCustomDetectionRuleOrgConfiguration, step task#3): status expected 400, got 200; body expected {'message': 'The request is rejected because the caller is not authorized to call this API.', '__type': 'InvalidInputException', 'type': 'InvalidInputException'}, got {} (empty_body); header x-amzn-errortype expected 'AccessDeniedException', got None.
Cause 2 (GetCustomDetectionRuleOrgConfiguration, step task#4): status expected 400, got 200; $.message expected 'The request is rejected because the caller is not authorized to call this API.', got None; $.__type expected 'InvalidInputException', got None; $.type expected 'InvalidInputException', got None; $headers.x-amzn-errortype expected 'AccessDeniedException', got None.
Cause 3 (UpdateCustomDetectionRuleOrgConfiguration, step task#5): status expected 400, got 200; body expected the same three-field InvalidInputException payload, got {}; x-amzn-errortype expected 'AccessDeniedException', got None.
Cause 4 (DeleteCustomDetectionRuleOrgConfiguration, step task#6): status expected 400, got 200; body expected the same three-field InvalidInputException payload, got {}; x-amzn-errortype expected 'AccessDeniedException', got None.
All four are single-step clusters (affected_count 1) from fixture guardduty-custom-detection-rule-org-configuration-errors in trace set simparity-wire-aws-v1.

## Steps to Reproduce
1. Replay fixture guardduty-custom-detection-rule-org-configuration-errors against the emulator as a caller that is NOT the organization's delegated administrator for GuardDuty.
2. Step task#3 (CreateCustomDetectionRuleOrgConfiguration): emulator answers 200 {} — expected 400 with header x-amzn-errortype 'AccessDeniedException' and body {'message': 'The request is rejected because the caller is not authorized to call this API.', '__type': 'InvalidInputException', 'type': 'InvalidInputException'}.
3. Step task#4 (GetCustomDetectionRuleOrgConfiguration): emulator answers 200 with the configuration projection — expected the same 400 refusal.
4. Step task#5 (UpdateCustomDetectionRuleOrgConfiguration): emulator answers 200 {} — expected the same 400 refusal.
5. Step task#6 (DeleteCustomDetectionRuleOrgConfiguration): emulator answers 200 {} — expected the same 400 refusal.
