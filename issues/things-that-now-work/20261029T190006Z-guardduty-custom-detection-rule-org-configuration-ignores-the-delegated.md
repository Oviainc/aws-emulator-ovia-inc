Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-custom-detection-rule-org-configuration-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-custom-detection-rule-org-configuration-errors guardduty-orgconfig-access-denied guardduty-association-lifecycle guardduty-association-notfound-errors guardduty-custom-detection-rule-association-errors
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->4xx|$.__type
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->4xx|$.message
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->4xx|$.type
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|CreateCustomDetectionRuleOrgConfiguration|4xx->4xx|status
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->4xx|$.__type
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->4xx|$.message
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->4xx|$.type
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|GetCustomDetectionRuleOrgConfiguration|4xx->4xx|status
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->4xx|$.__type
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->4xx|$.message
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->4xx|$.type
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|UpdateCustomDetectionRuleOrgConfiguration|4xx->4xx|status
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->4xx|$.__type
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->4xx|$.message
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->4xx|$.type
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|DeleteCustomDetectionRuleOrgConfiguration|4xx->4xx|status
Issue-Key: guardduty|ListCustomDetectionRuleOrgConfigurations|4xx->2xx|$.__type
Issue-Key: guardduty|ListCustomDetectionRuleOrgConfigurations|4xx->2xx|$.message
Issue-Key: guardduty|ListCustomDetectionRuleOrgConfigurations|4xx->2xx|$.type
Issue-Key: guardduty|ListCustomDetectionRuleOrgConfigurations|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|ListCustomDetectionRuleOrgConfigurations|4xx->2xx|status
Issue-Key: guardduty|GetCustomDetectionRuleAssociation|4xx->4xx|$.message
Issue-Key: guardduty|UpdateCustomDetectionRuleAssociation|4xx->4xx|$.message
Issue-Key: guardduty|DeleteCustomDetectionRuleAssociation|2xx->4xx|$
Issue-Key: guardduty|DeleteCustomDetectionRuleAssociation|2xx->4xx|status
Issue-Related: 20261029T190006Z-guardduty-custom-detection-rule-associations-carry-a-malformed-arn.md
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Custom-detection-rule org configuration ignores the delegated-admin authorization gate

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomDetectionRuleOrgConfiguration

## Description
The five CustomDetectionRuleOrgConfiguration operations have no authorization gate at all, so a caller that is not the organization's delegated administrator reaches resource resolution and is answered with a resource error instead of the authorization refusal; separately, the association not-found refusals use the wrong message and DeleteCustomDetectionRuleAssociation is not idempotent. 9 causes.

Shared AWS grounding for causes 1-5: the *CustomDetectionRuleOrgConfiguration operations are restricted to the organization's delegated administrator. A caller without that standing is refused before any rule or configuration is looked up: HTTP 400, header x-amzn-errortype 'AccessDeniedException', body $.__type and $.type 'InvalidInputException', message 'The request is rejected because the caller is not authorized to call this API.' — entirely fixed text. (The status/code pairing is unusual and is taken from the recorded responses, not inferred.)

Cause 1 of 9: CreateCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the refusal above, before the ruleId is resolved.
Emulator: HTTP 404 with x-amzn-errortype/__type/type 'ResourceNotFoundException' and 'The request is rejected because the input ruleId does not refer to a custom detection rule published by GuardDuty.'
Why: an unauthorized caller is told the rule does not exist; an operator chases a nonexistent catalog problem while the real fault is that the account is not the delegated admin, and any client that distinguishes authorization failures from resource failures mis-routes the error.
Fix: an authorization check in the CreateCustomDetectionRuleOrgConfiguration handler, ahead of rule resolution.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#3.

Cause 2 of 9: GetCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the same 400 / AccessDeniedException / fixed authorization message.
Emulator: HTTP 404, ResourceNotFoundException, 'The request is rejected because the input ruleId and mode do not refer to an organization configuration of the current account.'
Why: as cause 1.
Fix: an authorization check in the Get handler, ahead of configuration lookup.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#4.

Cause 3 of 9: UpdateCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the same refusal.
Emulator: HTTP 404, ResourceNotFoundException, 'The request is rejected because the input ruleId does not refer to a custom detection rule with an organization configuration of the current account.'
Why: as cause 1, on a mutating path — an unauthorized update is reported as a missing resource, so a retry loop keeps trying.
Fix: an authorization check in the Update handler.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#5.

Cause 4 of 9: DeleteCustomDetectionRuleOrgConfiguration has no authorization gate.
AWS: the same refusal.
Emulator: HTTP 404, ResourceNotFoundException, 'The request is rejected because the input ruleId and mode do not refer to an organization configuration of the current account.'
Why: as cause 3.
Fix: an authorization check in the Delete handler.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#6.

Cause 5 of 9: ListCustomDetectionRuleOrgConfigurations answers an unauthorized caller with a success.
AWS: the same 400 / AccessDeniedException refusal — the listing is not readable by a non-delegated-admin caller.
Emulator: HTTP 200 with no message, no __type, no type and no x-amzn-errortype.
Why: this is the most severe of the five — an unauthorized caller is handed organization configuration data instead of being refused, so an emulator-backed test of the delegated-admin restriction passes while the restriction does not exist.
Fix: an authorization check in the List handler.
Traces: guardduty-custom-detection-rule-org-configuration-errors#task#7.

Cause 6 of 9: GetCustomDetectionRuleAssociation not-found message.
AWS: HTTP 404, ResourceNotFoundException, message 'Association not found: <associationId>' — fixed prefix 'Association not found: ' plus the associationId from the request.
Emulator: 'The request is rejected because the input ruleId and associationId do not refer to a custom detection rule association of the current account.'
Why: AWS's message echoes back which association id was not found, which is what a caller logs and correlates; the emulator's text drops it and asserts an account-ownership fault instead.
Fix: the GetCustomDetectionRuleAssociation handler's ResourceNotFoundException message.
Traces: guardduty-association-lifecycle#teardown#9.

Cause 7 of 9: UpdateCustomDetectionRuleAssociation not-found message.
AWS: HTTP 404, ResourceNotFoundException, 'Association not found: <associationId>' with the id from the request.
Emulator: the same account-ownership sentence as cause 6.
Why: as cause 6.
Fix: the UpdateCustomDetectionRuleAssociation handler's message.
Traces: guardduty-association-notfound-errors#task#3.

Cause 8 of 9: DeleteCustomDetectionRuleAssociation is not idempotent.
AWS: deleting an association id that does not exist succeeds — HTTP 200 with an empty body ({}).
Emulator: HTTP 404 with {__type: 'ResourceNotFoundException', type: 'ResourceNotFoundException', message: 'The request is rejected because the input ruleId and associationId do not refer to a custom detection rule association of the current account.'}.
Why: idempotent delete is what teardown and reconciliation code relies on; a 404 on a delete turns a successful cleanup into a failure and aborts the surrounding workflow.
Fix: the DeleteCustomDetectionRuleAssociation handler — a missing association must not be an error.
Traces: guardduty-association-notfound-errors#task#4.

Cause 9 of 9: CreateCustomDetectionRuleAssociation unknown-rule message.
AWS: HTTP 404, ResourceNotFoundException, message 'Rule not found: <ruleId>' — fixed prefix 'Rule not found: ' plus the ruleId from the request.
Emulator: 'The request is rejected because the input ruleId does not refer to a custom detection rule published by GuardDuty.'
Why: as cause 6 — the rule id the caller asked for is dropped from the message.
Fix: the Create handler's rule-not-found message.
Traces: guardduty-association-notfound-errors#task#5.

Related open issue (does NOT cover these; it carries no replay key for them): 20261029T183757Z-guardduty-performs-no-authorization-identity-policy-deny-is.md.

## Evidence
Causes 1-4: status expected 400, got 404; x-amzn-errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'; $.__type/$.type expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message expected 'The request is rejected because the caller is not authorized to call this API.', got a per-operation 'does not refer to ... of the current account' sentence.
Cause 5: status expected 400, got 200; message/__type/type/x-amzn-errortype all expected present, got None.
Causes 6-7: $.message expected 'Association not found: <associationId>', got 'The request is rejected because the input ruleId and associationId do not refer to a custom detection rule association of the current account.' (status 404 both sides)
Cause 8: status expected 200 with body {}, got 404 with {'__type': 'ResourceNotFoundException', 'message': 'The request is rejected because the input ruleId and associationId do not refer to a custom detection rule association of the current account.', 'type': 'ResourceNotFoundException'}.
Cause 9: $.message expected 'Rule not found: <ruleId>', got 'The request is rejected because the input ruleId does not refer to a custom detection rule published by GuardDuty.'

## Steps to Reproduce
1. Replay guardduty-custom-detection-rule-org-configuration-errors steps task#3 through task#7 as a caller that is not the organization's delegated administrator: each of Create/Get/Update/Delete answers 404 ResourceNotFoundException and List answers 200, where all five must answer 400 with x-amzn-errortype AccessDeniedException.
2. Replay guardduty-association-lifecycle step teardown#9 and guardduty-association-notfound-errors step task#3 for the 'Association not found: <id>' wording.
3. Replay guardduty-association-notfound-errors step task#4: DeleteCustomDetectionRuleAssociation for an unknown id returns 404 instead of 200 {}.
4. Replay guardduty-association-notfound-errors step task#5 for the 'Rule not found: <ruleId>' wording.
