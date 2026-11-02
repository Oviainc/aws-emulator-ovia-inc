Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a399f03ccc4f5afa40c0af0ce737308e8fb96c4f

# Fold RegisterSlackWorkspaceForOrganization's inline registry-presence test into the state helper that already implements it

_Filed: 2026-10-29T16:48:24Z_
_Operation: RegisterSlackWorkspaceForOrganization | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a399f03ccc4f5afa40c0af0ce737308e8fb96c4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "is this `teamId` in the account's Slack workspace registry" is implemented twice in this crate. `crate::state::ComAmazonawsSupportappSupportappState::unregistered_workspace_conflict` owns it for the create path (`!self.slack_workspace_configurations.contains_key(team_id)`), and `CreateSlackChannelConfiguration` calls that helper. `RegisterSlackWorkspaceForOrganization` needs the SAME test to decide its documented member-account conflict and its quota accounting, but re-implements it inline against the map (`store.slack_workspace_configurations.contains_key(&input.team_id)`) — and its own comment asserts the two are the same test, so the crate already states the invariant that the second copy can silently break.

That is exactly the drift `state.rs`'s module doc says this module exists to prevent ("so the operation files agree on one implementation of each instead of carrying copies that can drift apart"): the state struct owns the registry and the presence rule, yet one of the two handlers that depend on the rule reaches past it into the map. If the registry ever grows a notion the presence test must honour (a per-workspace status, an organization-scoped key), the helper and the inline copy can be changed independently and the two handlers would then disagree about whether a workspace is registered.

The fix is a pure extraction: give the state struct the predicate the registry rule already is (used by `unregistered_workspace_conflict` for its message and by the register handler for its boolean) and have both call sites read it from there. No status, body, message or error shape changes, so every existing test stays green.

## Evidence

Copy 1 — the rule as the state module owns it, services/service-com-amazonaws-supportapp-supportapp/src/state.rs:276-279:
```
    pub fn unregistered_workspace_conflict(&self, team_id: &str) -> Option<String> {
        (!self.slack_workspace_configurations.contains_key(team_id))
            .then(|| workspace_not_registered_message(team_id))
    }
```
and the handler that consumes it, src/ops/create_slack_channel_configuration.rs:254:
```
        if let Some(message) = store.unregistered_workspace_conflict(&input.team_id) {
```

Copy 2 — the same rule re-implemented in the register handler, src/ops/register_slack_workspace_for_organization.rs:254-256:
```
        let already_registered = store
            .slack_workspace_configurations
            .contains_key(&input.team_id);
```
with the comment right below it, register_slack_workspace_for_organization.rs:259-262, asserting the two copies are the same test:
"The registry is what settles that — it holds the workspaces registered for this organization, the same presence test `CreateSlackChannelConfiguration` makes".

The stated reason the shared module exists, src/state.rs:5-11:
"Besides the state struct itself this module holds the few rules the Support App handlers share — ... so the operation files agree on one implementation of each instead of carrying copies that can drift apart."

Test coverage that pins both call sites and must stay unchanged:
- tests/all/channel_configuration_workspace_precondition.rs:52-59 and :99-122 pin the create-path refusal (409 `ConflictException`, message "Workspace installation not found for account {account} and team {team_id}") driven by copy 1, across five `teamId` forms including both published `len 1..256` bounds.
- tests/all/organization_account_role.rs:117-130, :162 (`a_member_account_cannot_register_a_workspace_the_organization_has_not_registered`) and :245 (`a_member_account_registers_a_workspace_the_management_account_registered`) pin the register-path decision driven by copy 2.
- tests/all/channel_configuration_workspace_precondition.rs:287-357 pins that `DeleteSlackWorkspaceConfiguration` removal makes the create path conflict again, i.e. that both copies read the same map.

## Steps to Reproduce

1. Read services/service-com-amazonaws-supportapp-supportapp/src/state.rs:5-11 — the module states that the rules the handlers share live here so the operation files cannot carry copies that drift apart.
2. Read src/state.rs:276-279 — `unregistered_workspace_conflict` implements the registry-presence rule (`contains_key` on `slack_workspace_configurations`) and is the only state-level entry point for it.
3. Read src/ops/create_slack_channel_configuration.rs:254 — the create path consults the registry exclusively through that helper.
4. Read src/ops/register_slack_workspace_for_organization.rs:254-256 — the register path re-implements the same `contains_key` test inline, and its comment at :259-262 claims it is "the same presence test `CreateSlackChannelConfiguration` makes", so the two copies are asserted equal while being written twice.
5. Confirm the extraction is response-neutral: run tests/all/channel_configuration_workspace_precondition.rs, tests/all/organization_account_role.rs and tests/all/slack_workspace_configurations.rs — statuses, error types and messages are produced by the unchanged message helpers, so all rows must still pass after both call sites read one predicate.
