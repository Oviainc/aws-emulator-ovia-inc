Issue-Source: trace-sweep-divergence
Issue-Trace: network-security-manager-policy-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-security-manager-policy-validation network-security-manager-rule-identifier-validation network-security-manager-scope-and-template-crud
Issue-Shape-Id: com.amazonaws.networksecuritymanager#PiccoloCustomerAPIService
Issue-Key: network-security-manager|CreatePolicy|4xx->2xx|$.fieldList
Issue-Key: network-security-manager|CreatePolicy|4xx->2xx|$.message
Issue-Key: network-security-manager|CreatePolicy|4xx->2xx|$.reason
Issue-Key: network-security-manager|CreatePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: network-security-manager|CreatePolicy|4xx->2xx|status
Issue-Key: network-security-manager|CreateRule|4xx->4xx|$.message
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.scopeArn
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.scopeConfiguration
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.scopeId
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.scopeName
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.status
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.updateToken
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.updatedAt
Issue-Key: network-security-manager|CreateScope|2xx->5xx|$.version
Issue-Key: network-security-manager|CreateScope|2xx->5xx|status
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# network-security-manager Create* entity operations diverge: 3 causes — WAF policy config never validated, rule-name pattern message mis-rendered, CreateScope answers 501

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
Three distinct, net-new defects in the Network Security Manager (PiccoloCustomerAPIService, restJson1) entity-creation surface. All three were observed replaying the simparity-wire-aws-v1 wire corpus against the emulator; none is owned by an open issue (the only related open issue, '20261109T113448Z-network-security-manager-createrule-stub.md' — "Implement network-security-manager CreateRule: unimplemented scaffold stub" — does not cover Cause 2: CreateRule now DOES answer and answers with the wrong validation message text, and the replay reports no owner and no stub for any of these steps).

=== Cause 1 of 3: CreatePolicy accepts a WAF policy whose wafConfig omits the required existingCustomerWebACLResolution ===
AWS: Smithy model com.amazonaws.networksecuritymanager#CreatePolicy — input member policyConfiguration targets PolicyConfiguration, whose member wafConfig targets WafConfig; WafConfig.existingCustomerWebACLResolution is @required (enum ExistingCustomerWebACLResolution, values RETROFIT | OVERRIDE_ASSOCIATION | NO_REMEDIATION), as is WafConfig.conflictResolution. CreatePolicy models ValidationException (@error client, @httpError 400) with required member `message`, member `reason` (enum ValidationExceptionReason, values ACCOUNT_NOT_ONBOARDED | FIELD_VALIDATION_FAILED | OTHER) and member `fieldList` (list of ValidationExceptionField {name, message}, both required). For a CreatePolicy whose firewallType is WAF but whose policyConfiguration.wafConfig does not supply existingCustomerWebACLResolution, the recorded AWS response is HTTP 400 with reason "FIELD_VALIDATION_FAILED", message "existingCustomerWebACLResolution is required for WAF policies", an x-amzn-errortype response header naming the error shape, and one fieldList entry whose name is the dotted path of the offending member ("policyConfiguration.wafConfig.existingCustomerWebACLResolution") and whose message is "existingCustomerWebACLResolution must be specified when firewallType is WAF. Valid values: RETROFIT, OVERRIDE_ASSOCIATION, NO_REMEDIATION". In that fieldList message the fixed text is "existingCustomerWebACLResolution must be specified when firewallType is <firewallType>. Valid values: <comma-separated ExistingCustomerWebACLResolution enum values>": the member name and the dotted field path come from the request's own structure and the firewall type comes from the request's firewallType member; the valid-values list is the modeled enum, not a per-run value.
Emulator: answers HTTP 201 and creates the policy. The whole error body is absent — no $.message, no $.reason, no $.fieldList, and no x-amzn-errortype header — i.e. the WAF-specific required-member/conditional check is simply not performed.
Why: a client that relies on AWS refusing an incomplete WAF policy gets a policy object back that AWS would never have created, so the invalid configuration is persisted and the client's error path (inspecting reason/fieldList to tell the user which field is missing) is never exercised. Downstream deployment of such a policy then behaves nothing like AWS.
Fix: CreatePolicy's input validation (the required/conditional-member check for PolicyConfiguration.wafConfig in the CreatePolicy handler's validation stage) must reject the request before any state mutation, and the ValidationException serializer must emit message, reason and fieldList plus the x-amzn-errortype header.
Traces: network-security-manager-policy-validation#task#1 (cluster also covers task#2 and task#3 of the same fixture).

=== Cause 2 of 3: CreateRule's pattern-constraint ValidationException message embeds the anchored regex instead of AWS's unanchored rendering ===
AWS: Smithy model shape RuleName — string, @length(min 1, max 128), @pattern "^[a-zA-Z0-9][a-zA-Z0-9 _.:/=+\\-@]*$"; CreateRuleInput.ruleName targets it and is @required. When ruleName violates the pattern, AWS answers HTTP 400 ValidationException whose message is the standard constraint report: "1 validation error detected: Value '<the rejected value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", and the <pattern> AWS renders is the expression WITHOUT the leading ^ and trailing $ anchors — i.e. "[a-zA-Z0-9][a-zA-Z0-9 _.:/=+\\-@]*". Fixed text: "N validation error(s) detected: Value '...' at '...' failed to satisfy constraint: Member must satisfy regular expression pattern: ..."; variable parts: the error count, the rejected value and the member name, which come from the request, and the pattern, which comes from the model.
Emulator: status is correct (400) but the message is "1 validation error detected: Value '_sim2real-probe-nsm-bad-name' at 'ruleName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9][a-zA-Z0-9 _.:/=+\\-@]*$" — the anchors are left in, so the message differs from AWS's for every pattern-constrained member.
Why: the message is part of the contract; tests and user-facing error surfaces that match AWS's constraint text (or echo it to an operator) see a different string, and because the defect is in the shared constraint-message renderer it mis-words every @pattern refusal in the service, not just ruleName.
Fix: the pattern-constraint message renderer used by CreateRule's input validation (shared constraint-violation formatting) must strip the anchoring ^ and $ when interpolating a @pattern into the "Member must satisfy regular expression pattern:" clause.
Traces: network-security-manager-rule-identifier-validation#task#1.

=== Cause 3 of 3: CreateScope is not served — 501 instead of creating the scope ===
AWS: Smithy model CreateScope — POST /scopes, success @http code 201, output CreateScopeOutput with required members scopeId, scopeArn, scopeName, status (EntityStatus DRAFT|ACTIVE) and version (EntityVersion, pattern ^[1-9][0-9]*$), plus scopeConfiguration, updateToken (36-char UUID form) and updatedAt (@timestampFormat date-time). A CreateScope with a valid scopeName and a scopeConfiguration whose resourceScopes selects a resource type returns 201 echoing the submitted scopeName and scopeConfiguration, status ACTIVE when isPublished defaults to true, version "1" for a first create, plus a freshly generated scopeId/scopeArn/updateToken/updatedAt. Modeled failure modes are the client/server errors on the operation (ValidationException, ConflictException, ...); 501 is not among them.
Emulator: answers HTTP 501 with no body — no scopeId, scopeArn, scopeName, scopeConfiguration, status, version, updateToken or updatedAt. The operation is routed but unimplemented, and because it is the entry point of the scope lifecycle every later step of the CRUD fixture (get/list/update/snapshot/delete of that scope) has nothing to act on.
Why: no client can create a scope, so the accounts-and-resources selection a deployment needs cannot be expressed at all; a caller sees a protocol-level 501 that no SDK models as a retriable or handleable NSM error, instead of a created scope or a modeled 4xx.
Fix: implement the CreateScope handler and its state (scope entity store: id/arn generation, version and updateToken issuance, DRAFT vs ACTIVE from isPublished, updatedAt) and its 201 serializer.
Traces: network-security-manager-scope-and-template-crud#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). 3 divergence clusters, all service=network-security-manager, all with empty stub_path and empty owned_by.

Cause 1 — fixture network-security-manager-policy-validation, step task#1, operation CreatePolicy, verdict mismatch, affected_count 3 (task#1, task#2, task#3):
  expected status 400, actual 201
  $.message: expected 'existingCustomerWebACLResolution is required for WAF policies', got None
  $.reason: expected 'FIELD_VALIDATION_FAILED', got None
  $.fieldList: expected [{'name': 'policyConfiguration.wafConfig.existingCustomerWebACLResolution', 'message': 'existingCustomerWebACLResolution must be specified when firewallType is WAF. Valid values: RETROFIT, OVERRIDE_ASSOCIATION, NO_REMEDIATION'}], got None
  $headers.x-amzn-errortype: expected (shaped, not asserted), got None

Cause 2 — fixture network-security-manager-rule-identifier-validation, step task#1, operation CreateRule, verdict mismatch (status 400 both):
  $.message expected "1 validation error detected: Value '_sim2real-probe-nsm-bad-name' at 'ruleName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9 _.:/=+\\-@]*"
  $.message actual   "1 validation error detected: Value '_sim2real-probe-nsm-bad-name' at 'ruleName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9][a-zA-Z0-9 _.:/=+\\-@]*$"
  (difference is exactly the leading ^ and trailing $ around the interpolated @pattern)

Cause 3 — fixture network-security-manager-scope-and-template-crud, step task#6, operation CreateScope, verdict mismatch:
  expected status 201, actual 501
  $.scopeName: expected the submitted scope name, got None
  $.scopeConfiguration: expected {'resourceScopes': {'AWS::EC2::EIP': {'includeAll': True}}}, got None
  $.status: expected 'ACTIVE', got None
  $.version: expected '1', got None
  $.scopeId / $.scopeArn / $.updateToken / $.updatedAt: expected (shaped, not asserted), got None

Model citations: CreatePolicy/CreateRule/CreateScope POST /policies|/rules|/scopes with success code 201; WafConfig.existingCustomerWebACLResolution and .conflictResolution @required; ExistingCustomerWebACLResolution = [RETROFIT, OVERRIDE_ASSOCIATION, NO_REMEDIATION]; ValidationExceptionReason = [ACCOUNT_NOT_ONBOARDED, FIELD_VALIDATION_FAILED, OTHER]; ValidationException @httpError 400 with required message + reason + fieldList; RuleName @pattern ^[a-zA-Z0-9][a-zA-Z0-9 _.:/=+\\-@]*$ (length 1..128); CreateScopeOutput requires scopeId, scopeArn, scopeName, status, version.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout (replay_wire_corpus), or exercise the three steps directly against the NSM endpoint:

Cause 1: POST /policies with firewallType=WAF, a valid policyName and priority, and policyConfiguration={remediationEnabled, resourcesCleanUp, wafConfig:{conflictResolution:...}} omitting wafConfig.existingCustomerWebACLResolution. Expect 400 ValidationException with reason FIELD_VALIDATION_FAILED, message 'existingCustomerWebACLResolution is required for WAF policies', a fieldList entry for policyConfiguration.wafConfig.existingCustomerWebACLResolution, and an x-amzn-errortype header; observe 201 and a created policy. (Fixture network-security-manager-policy-validation, steps task#1-task#3.)

Cause 2: POST /rules with ruleName='_sim2real-probe-nsm-bad-name' (leading underscore violates RuleName's @pattern) plus a valid firewallType/ruleType/configuration. Expect 400 whose message ends '...Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9 _.:/=+\\-@]*'; observe the same message with the ^ and $ anchors retained. (Fixture network-security-manager-rule-identifier-validation, step task#1.)

Cause 3: POST /scopes with a valid scopeName and scopeConfiguration={'resourceScopes': {'AWS::EC2::EIP': {'includeAll': true}}}. Expect 201 with scopeId/scopeArn/scopeName/scopeConfiguration/status=ACTIVE/version='1'/updateToken/updatedAt; observe 501 with no body. (Fixture network-security-manager-scope-and-template-crud, step task#6.)
