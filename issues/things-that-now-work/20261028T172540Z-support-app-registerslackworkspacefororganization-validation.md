Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 62eaf53b582c9d7bec5520c65fe502b1fdb205cf

# RegisterSlackWorkspaceForOrganization ignores the account role it computes: a member-account registration is never refused and always grants allowOrganizationMemberAccount

_Filed: 2026-10-28T17:25:40Z_
_Operation: RegisterSlackWorkspaceForOrganization | Class: validation_

> _Note: this issue was identified against the code base at commit `62eaf53b582c9d7bec5520c65fe502b1fdb205cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler reads the caller's role in its organization from the Organizations store (`management` vs `member`) and then uses that value for nothing but filling the response. Two consequences:

1. The second `ConflictException` condition the model documents for this operation — a call from a member account for a workspace the management account has not registered for the organization — is never raised. The handler's only refusals are "account belongs to no organization" and the workspace quota; a caller whose computed `accountType` is `member` is served HTTP 200 and the workspace is inserted into the registry, which is the opposite of the documented conflict. The crate already holds everything the check needs: `caller_account_type` returns `ACCOUNT_TYPE_MEMBER`, and "has the management account registered that workspace yet" is exactly a presence test on `slack_workspace_configurations` — the same registry test `CreateSlackChannelConfiguration` already makes through `unregistered_workspace_conflict`.

2. `allow_organization_member_account` is set to `Some(true)` on every successful call, whatever the computed role, although the handler's own module doc explains that flag as what a registration *from the management account* grants. A member-account registration therefore records the organization-wide grant that only the management account's registration represents, and `ListSlackWorkspaceConfigurations` reports it.

Both symptoms are the same root defect — the computed `account_type` governs no rule — so they are one finding on this operation.

## Evidence

Smithy (services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json), `smithy_query(query="doc",shape="ConflictException")` — the error this operation declares with httpError 409 enumerates:
  "Call the RegisterSlackWorkspaceForOrganization API from a member account, but the management account hasn't registered that workspace yet for the organization."
and `smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")` states the two-step model that condition protects:
  "After the management account authorizes the Slack workspace, member accounts can call this API to authorize the same Slack workspace for their individual accounts."

Handler — services/service-com-amazonaws-supportapp-supportapp/src/ops/register_slack_workspace_for_organization.rs:215 computes the role:
  "let account_type = crate::state::caller_account_type(&*self.engine)"
and the only refusals after it are the no-organization case (:223-231) and the quota (:242-257). The insert at :259-270 and then :276
  "record.allow_organization_member_account = Some(true);"
run for every caller; `account_type` is consumed only by the response at :284 ("account_type: Some(account_type.to_string())"). No branch anywhere in the handler reads it.

Internal contradiction inside this crate: the same file's module doc, register_slack_workspace_for_organization.rs:29-32, states the rule the code does not apply —
  "The stored record's `allowOrganizationMemberAccount` is set on every successful call, including one for a workspace the account already held: registering a workspace for the organization from its management account is what lets member accounts authorize the same workspace."
while src/state.rs:83 defines the other role the handler can compute ("pub const ACCOUNT_TYPE_MEMBER: &str = \"member\";") and src/state.rs:129-131 returns it ("} else if organizations.accounts.contains_key(caller) { Ok(Some(ACCOUNT_TYPE_MEMBER))").

Test coverage: tests/all/organization_membership.rs covers only the no-organization conflict and the reported `accountType` value ("the precondition is membership of an organization", lines 3-27); tests/all/slack_workspace_configurations.rs:154-159 pins `allowOrganizationMemberAccount == true` for a management-account registration only. No test exercises a caller whose computed role is `member`, so nothing pins the behaviour this issue reports.

## Steps to Reproduce

1. Read services/service-com-amazonaws-supportapp-supportapp/src/ops/register_slack_workspace_for_organization.rs:195-286: `account_type` is computed at :215 and is referenced again only at :284, inside the output struct.
2. Arrange Organizations state in which `crate::state::caller_account_type` returns `Some(ACCOUNT_TYPE_MEMBER)` — an organization whose `master_account_id` is not the emulator account while `accounts` holds it (src/state.rs:123-133).
3. POST /control/register-slack-workspace-for-organization with {"teamId": "T012ABCDEFG"} for a teamId absent from `slack_workspace_configurations` (i.e. no management-account registration exists).
4. Observed: HTTP 200, the workspace is inserted and `allow_organization_member_account` is set to true, so ListSlackWorkspaceConfigurations reports the organization-wide grant.
5. Expected per the documented condition: the modelled ConflictException (HTTP 409) for "a member account, but the management account hasn't registered that workspace yet for the organization", with nothing registered.
