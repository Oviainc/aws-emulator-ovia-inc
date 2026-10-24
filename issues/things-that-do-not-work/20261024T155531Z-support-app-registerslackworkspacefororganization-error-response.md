Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Refuse RegisterSlackWorkspaceForOrganization with ConflictException when the account belongs to no organization

_Filed: 2026-10-24T15:55:31Z_
_Operation: RegisterSlackWorkspaceForOrganization | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RegisterSlackWorkspaceForOrganization is documented as an Organizations-scoped call: "Registers a Slack workspace for your Amazon Web Services account. To call this API, your account must be part of an organization in Organizations." The modeled ConflictException (HTTP 409) explicitly enumerates the failure mode: "Call the RegisterSlackWorkspaceForOrganization API from an Amazon Web Services account that doesn't belong to an organization."

Expected: when the calling account is not a member of an AWS Organization (no organization exists for it), the call is refused with ConflictException, HTTP 409, and nothing is registered — so ListSlackWorkspaceConfigurations stays empty and CreateSlackChannelConfiguration still hits its missing-workspace precondition. Only once the account is in an organization does the registration succeed, and the reported accountType reflects whether the caller is the organization's management account or a member account (AccountType = management | member).

Actual: the handler never consults any organization state. It goes straight to the workspace registry, inserts/updates the record and answers HTTP 200 with a hardcoded accountType of "management" (crate::state::ACCOUNT_TYPE = "management", documented in src/state.rs as "the emulator presents exactly ONE account … so that account is the management account of its own organization"). The emulator's own Organizations service models the organization as absent until CreateOrganization is called (service-com-amazonaws-organizations-awsorganizationsv20161128/src/state.rs: `pub organization: Option<OrganizationRecord>`, with AWSOrganizationsNotInUseException for requests made with none), so on a fresh emulator the account provably belongs to no organization and the call should be a 409 — yet it succeeds. The modeled ConflictException is therefore unreachable for this operation and the accountType member can never report "member".

This is distinct from the already-filed Register gap about never-authorized Slack workspaces (which asks for ResourceNotFoundException/404 from an authorized-workspace lookup): this one is the organization-membership precondition that the model binds to ConflictException/409, plus the accountType value that follows from it.

## Evidence

Spec: smithy_query(query="doc", shape="RegisterSlackWorkspaceForOrganization") — "To call this API, your account must be part of an organization in Organizations"; smithy_query(query="doc", shape="ConflictException") — "Call the RegisterSlackWorkspaceForOrganization API from an Amazon Web Services account that doesn't belong to an organization" and "… from a member account, but the management account hasn't registered that workspace yet for the organization". The operation models ConflictException (HTTP 409) and output member accountType (AccountType: management | member).

Handler: services/service-com-amazonaws-supportapp-supportapp/src/ops/register_slack_workspace_for_organization.rs, handle() lines ~188-251 — it locks the store, checks only MAX_SLACK_WORKSPACE_CONFIGURATIONS, inserts the record and returns account_type: Some(crate::state::ACCOUNT_TYPE) . RegisterSlackWorkspaceForOrganizationErrorModeled::ConflictException is declared (line ~136) but never constructed anywhere in the crate.

Cross-service state available: services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/state.rs line 31 `pub organization: Option<OrganizationRecord>` (plus `accounts`), and the workspace already shows cross-crate service deps are permitted (Cargo.toml workspace deps).

Tests: tests/all/resource_preconditions.rs::registering_a_workspace_reports_it_and_is_bounded_by_the_documented_quota asserts a bare 200 with accountType == ACCOUNT_TYPE for any teamId; no test covers an account outside an organization, and no test asserts a 409 from this operation.

## Steps to Reproduce

1. Start the emulator with a fresh state and do NOT call organizations:CreateOrganization, so the calling account belongs to no AWS Organization (the Organizations emulator answers AWSOrganizationsNotInUseException for describe calls in this state).
2. Call supportapp RegisterSlackWorkspaceForOrganization with teamId="T012ABCDEFG".
3. Observe the emulator answers HTTP 200 with {"teamId":"T012ABCDEFG","accountType":"management"}; real AWS refuses the call with ConflictException (HTTP 409, x-amzn-errortype: ConflictException) because the account does not belong to an organization.
4. Call ListSlackWorkspaceConfigurations: the emulator reports the workspace that should never have been registered.
5. Call organizations:CreateOrganization and repeat step 2 — only now should the registration succeed, and accountType should be derived from the account's role in the organization (management for the org's management account, member otherwise) rather than hardcoded.
