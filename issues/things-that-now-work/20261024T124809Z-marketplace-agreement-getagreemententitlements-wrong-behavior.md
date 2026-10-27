Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# Every entitlement is reported without resource, type or registrationToken

_Filed: 2026-10-24T12:48:09Z_
_Operation: GetAgreementEntitlements | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AgreementEntitlement "represents an entitlement associated with an agreement, including the provisioning status, RESOURCE, and TYPE". Expected: each entitlement names the resource it was provisioned to (Resource$id/$type, e.g. the product), its entitlement type, and — for purchase agreements — a registrationToken.

Actual: AcceptAgreementRequest provisions entitlements from proposal_summary.resources, which is populated only by inheritance from a source agreement, so for every NEW agreement it is None and State::provision_entitlements() stores a single record with resource: None. Agreement::to_entitlements() then hard-codes `r#type: None` and `registration_token: None`. So GetAgreementEntitlements always answers with entitlements carrying only status, statusReasonCode and licenseArn — three of the six modelled members are permanently absent, and SearchAgreements' AgreementViewSummary$entitlements likewise carries licence ARNs only. A client that resolves an entitlement to the product it entitles, or that registers with a provider using the token, has nothing to read.

## Evidence

Spec: AgreementEntitlement$resource ("The resource that the entitlement is provisioned to, such as a product"), $type ("The type of entitlement"), $registrationToken ("A short-lived token required by acceptors to register their account with the product provider... only applicable for purchase agreements" — and every agreement this emulator mints is a PurchaseAgreement, state::AGREEMENT_TYPE). Handler/state: src/ops/get_agreement_entitlements.rs returns agreement.to_entitlements(now); src/state.rs to_entitlements() sets r#type: None and registration_token: None, provision_entitlements() stores resource: None when the proposal names no resource. Tests: tests/all/agreement_entitlements.rs:206 asserts resource, type and registrationToken are all null, i.e. the divergent behavior is pinned by a test and would need updating.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[...]) then AcceptAgreementRequest to mint an agreement
2. GetAgreementEntitlements(agreementId=<id>) -> observe agreementEntitlements[0] = {status, statusReasonCode, licenseArn} with resource, type and registrationToken all absent
3. SearchAgreements(filters=[{name:"AgreementType",values:["PurchaseAgreement"]}]) -> the summary's entitlements[] carries only licenseArn
