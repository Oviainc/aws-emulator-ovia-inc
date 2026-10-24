Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# Populate teamName for registered Slack workspaces instead of always omitting it

_Filed: 2026-10-24T15:14:55Z_
_Operation: RegisterSlackWorkspaceForOrganization | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modeled output member `teamName` ("The name of the Slack workspace") is never produced by this emulator. `RegisterSlackWorkspaceForOrganization` stores `team_name: None` unconditionally, and no other operation in this service can ever write it, so:

- `RegisterSlackWorkspaceForOrganization` always answers `{"teamId": ..., "accountType": "management"}` with `teamName` absent.
- Every `SlackWorkspaceConfiguration` record reported by `ListSlackWorkspaceConfigurations` is likewise missing `teamName` forever.

Expected (real service): a registered/authorized Slack workspace has a name in Slack, and the Support App reports it — the register response and each listed workspace configuration carry `teamName` alongside `teamId` and `allowOrganizationMemberAccount`. A client that renders the workspace name (console/CLI output, an integration test comparing against a recording) sees an empty value against the emulator where AWS returns a name.

Actual (emulator): the member is hardcoded absent, so the field is dead on the wire for every workspace. Since the emulator has no Slack to query, the fix is to synthesize and PERSIST a deterministic workspace name at registration time (derived from the `teamId`) so it is stable across calls and reported identically by register and by the listing — rather than leaving a modeled member that can never have a value.

## Evidence

Spec: `RegisterSlackWorkspaceForOrganizationResult.teamName` (target `teamName`, len 1..256, pattern `^.+$`) documented as "The name of the Slack workspace" and `SlackWorkspaceConfiguration.teamName` with the same doc (smithy_query query="shape" shape="SlackWorkspaceConfiguration" / query="doc" shape="RegisterSlackWorkspaceForOrganizationResult" member="teamName"). AWS's own API/CLI examples for both operations show `teamName` returned next to `teamId`.

Handler: `src/ops/register_slack_workspace_for_organization.rs` lines ~229-250 insert `crate::state::SlackWorkspaceConfig { team_id, team_name: None, allow_organization_member_account: None }` with the comment "The workspace's Slack name is not in the request and this emulator has no Slack to ask, so the optional member stays absent", then return `team_name: record.team_name.clone()` (always `None`). `src/state.rs` `SlackWorkspaceConfig::to_modeled` propagates that `None` into every listing record.

Test coverage: the current suites assert the ABSENCE of the member, i.e. they pin the divergence rather than the AWS behavior — `tests/all/resource_preconditions.rs` lines 293-299 ("expected no teamName") and `tests/all/slack_workspace_configurations.rs` lines 198-206 ("expected no teamName"). No test asserts a reported workspace name, so the AWS-visible field is untested and unimplemented.

## Steps to Reproduce

1. POST /control/register-slack-workspace-for-organization with body {"teamId": "T012ABCDEFG"}
2. Observe the 200 response body: it contains `teamId` and `accountType` but no `teamName` (AWS returns the Slack workspace name here)
3. POST /control/list-slack-workspace-configurations with body {}
4. Observe the single `slackWorkspaceConfigurations` entry: `teamId` and `allowOrganizationMemberAccount` are present, `teamName` is absent on every record and in every subsequent call
