Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# ListIdentitySources never reports ApprovalPortalUrl for an IAM Identity Center source

_Filed: 2026-10-24T15:31:46Z_
_Operation: ListIdentitySources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: for an identity source of type IAM_IDENTITY_CENTER, ListIdentitySources reports IdentitySourceParameters.IamIdentityCenter with all three members the model declares — InstanceArn, Region and ApprovalPortalUrl ("URL for the approval portal associated with the IAM Identity Center instance"). The approval portal URL is how approvers reach a pending session, so the real service returns it for every active IAM Identity Center source.

Actual: the handler hard-codes `approval_portal_url: None`, so the member is omitted from every entry of every page, for every identity source, no matter how long it has been ACTIVE. A client that reads the portal URL out of ListIdentitySources (the only non-stub read of an identity source in this crate — GetIdentitySource is still a scaffold) finds nothing and cannot distinguish "no portal" from "source not ready". Nothing else in the crate ever sets or stores a portal URL: crate::state::IdentitySource records only arn/source_type/instance_arn/instance_region/tags/status, so the field is structurally unreachable rather than conditionally absent.

## Evidence

Smithy model: com.amazonaws.mpa#IamIdentityCenterForList declares ApprovalPortalUrl (String, length max 1000) with doc "URL for the approval portal associated with the IAM Identity Center instance"; it is the member of IdentitySourceParametersForList that ListIdentitySources$IdentitySources carries. Handler: services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_identity_sources.rs, fn summary() — `approval_portal_url: None` with a comment stating the emulator serves no portal, while InstanceArn and Region are read off the stored record. State: src/state.rs IdentitySource has no portal field at all. Tests: tests/all/identity_sources.rs asserts the listed members but never asserts a portal URL (grep for ApprovalPortalUrl/approval_portal_url in tests returns no hits), so no test covers or contradicts this.

## Steps to Reproduce

1. Record the caller's account as the organization management account in the service store (CreateIdentitySource is gated on it, as tests/all/identity_sources.rs does), then POST /identity-sources with IdentitySourceParameters.IamIdentityCenter = { InstanceArn: "arn:aws:sso:::instance/ssoins-1234567890abcdef", Region: "us-east-1" }.
2. Wait past state::IDENTITY_SOURCE_ACTIVATION_DELAY so the source is reported ACTIVE.
3. POST /identity-sources/?List and read IdentitySources[0].IdentitySourceParameters.IamIdentityCenter.
4. Observe InstanceArn and Region present but ApprovalPortalUrl absent; real AWS returns an approval-portal URL for the instance.
