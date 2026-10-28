// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the role the organization gives the caller GOVERNING
//! `RegisterSlackWorkspaceForOrganization`, not just being reported by it.
//!
//! The rule under test: a Slack workspace is authorized for an organization in two steps
//! — "After the management account authorizes the Slack workspace, member accounts can
//! call this API to authorize the same Slack workspace for their individual accounts"
//! (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`) — so a
//! registration from an account that is only a MEMBER of its organization is served for a
//! workspace the organization already has registered and refused, with the modelled
//! `ConflictException` (HTTP 409, the condition that error enumerates: "Call the
//! RegisterSlackWorkspaceForOrganization API from a member account, but the management
//! account hasn't registered that workspace yet for the organization",
//! `smithy_query(query="doc",shape="ConflictException")`), for one it does not; and
//! `allowOrganizationMemberAccount` — "whether to allow member accounts to authorize
//! Slack workspaces"
//! (`smithy_query(query="doc",shape="SlackWorkspaceConfiguration",member="allowOrganizationMemberAccount")`)
//! — is recorded by the MANAGEMENT account's registration, so no call from a member
//! account records it.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]: the
//! organization is created by driving the Organizations emulator's own
//! `CreateOrganization`, and which account manages it afterwards is read back over the
//! wire with `DescribeOrganization`, so a response that is not computed from persisted
//! state cannot pass.
//!
//! The tests are tables. Their rows differ along what the rule generalises over: which
//! account manages the organization (so the caller's role is read per call rather than
//! fixed), the `teamId` form and length (the spread
//! [`crate::slack_workspace_configurations`] registers, covering both published
//! `len 1..256` bounds), how many workspaces the organization already has registered, and
//! whether the workspace in the request is one of them.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    derived_slack_workspace_name, ACCOUNT_TYPE_MANAGEMENT, ACCOUNT_TYPE_MEMBER,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};
use crate::slack_workspace_configurations::{
    list_workspaces, reported, reported_team_ids, team_ids,
};

/// The operation under test, as its refusals name it.
const OPERATION: &str = "RegisterSlackWorkspaceForOrganization";

/// The management accounts the tables hand the organization to. Each is a 12-digit
/// account id other than the one the emulator presents, so the calling account is a
/// member of the organization rather than the account that manages it; rows differ in
/// how far the id is from the caller's, down to a single digit.
fn other_management_accounts() -> Vec<String> {
    let caller = engine::emulator_account_id();
    let mut digits: Vec<char> = caller.chars().collect();
    let last = digits.len() - 1;
    digits[last] = if digits[last] == '9' { '8' } else { '9' };
    let nearly_the_caller: String = digits.into_iter().collect();

    let accounts = vec![
        "111122223333".to_string(),
        "999988887777".to_string(),
        nearly_the_caller,
    ];
    for account in &accounts {
        assert_ne!(
            account,
            caller,
            "a management account row must be an account other than the caller {caller}"
        );
        assert!(
            account.len() == 12 && account.chars().all(char::is_numeric),
            "a management account row must be a 12-digit account id, got {account:?}"
        );
    }
    accounts
}

/// Register `team_id` with whatever role the organization currently gives this account —
/// the raw call, with no organization arranged first (the harness's own
/// `register_workspace` creates one, which would make the caller its management account).
async fn register(emulator: &Emulator, team_id: &str) -> Reply {
    emulator
        .call(
            "register-slack-workspace-for-organization",
            json!({ "teamId": team_id }),
        )
        .await
}

/// An organization this account is a MEMBER of: created by this account, then handed to
/// `master_account_id`. The hand-over is read back with `DescribeOrganization`, so the
/// role every later call computes is the one the Organizations store reports.
async fn organization_managed_by(emulator: &Emulator, label: &str, master_account_id: &str) {
    emulator.hand_organization_to(master_account_id);
    let described = emulator
        .organizations("DescribeOrganization", json!({}))
        .await;
    assert_eq!(
        described.status, 200,
        "case {label:?}: DescribeOrganization must answer HTTP 200, got {} / {}",
        described.status, described.body
    );
    assert_eq!(
        described.body["Organization"]["MasterAccountId"].as_str(),
        Some(master_account_id),
        "case {label:?}: expected the organization to be managed by {master_account_id}, \
         got {}",
        described.body
    );
}

/// Assert `reply` is the modelled `ConflictException` for a member account's registration
/// of a workspace the management account has not registered: HTTP 409, the error type on
/// the wire, and a message that names the account the emulator presents, the workspace and
/// the operation that was called.
fn assert_workspace_not_authorized(label: &str, team_id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 409,
        "case {label:?}: expected HTTP 409, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ConflictException",
        "case {label:?}: expected x-amzn-errortype ConflictException, got {:?}",
        reply.error_type
    );
    let message = reply.string("message").unwrap_or_default();
    let account = engine::emulator_account_id();
    assert!(
        message.contains(account)
            && message.contains(team_id)
            && message.contains(OPERATION)
            && message.contains("member account")
            && message.contains("management account"),
        "case {label:?}: expected a message naming account {account}, workspace {team_id}, \
         the {OPERATION} API and both account roles, got {message:?}"
    );
}

/// The record the listing reports for `team_id`, or `None` when the registry holds none.
async fn listed_workspace(emulator: &Emulator, team_id: &str) -> Option<Value> {
    reported(&list_workspaces(emulator, None).await)
        .into_iter()
        .find(|record| record.get("teamId").and_then(Value::as_str) == Some(team_id))
}

/// A channel configuration in `team_id`, which is only possible once that workspace is
/// registered — the independent read of "is this workspace in the registry at all".
async fn create_channel(emulator: &Emulator, team_id: &str) -> Reply {
    emulator
        .create(json!({
            "teamId": team_id,
            "channelId": "C01234A5BCD",
            "notifyOnCaseSeverity": "none",
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await
}

#[tokio::test]
async fn a_member_account_cannot_register_a_workspace_the_organization_has_not_registered() {
    let all = team_ids();
    for master in other_management_accounts() {
        // Rows: how many workspaces the organization already has registered before the
        // caller becomes a member account — none, one, several — so each refusal is about
        // the workspace in the request and not about an empty registry.
        for already in [0usize, 1, 3] {
            let target = all
                .last()
                .expect("the team id table must not be empty")
                .clone();
            let label = format!(
                "managed by {master}, {already} already registered, team {}",
                &target[..target.len().min(16)]
            );
            let emulator = Emulator::new();
            for team_id in all.iter().take(already) {
                assert_eq!(
                    emulator.register_workspace(team_id).await.status,
                    200,
                    "case {label:?}: workspace {} must be registered by the management \
                     account",
                    &team_id[..team_id.len().min(16)]
                );
            }
            // The organization has to exist before it can be handed over, so a row that
            // registers nothing still creates it.
            emulator.ensure_organization().await;
            organization_managed_by(&emulator, &label, &master).await;

            let before = list_workspaces(&emulator, None).await;

            // Twice: a refusal is not a state transition, so the second call is refused
            // for the same reason as the first.
            for attempt in 1..=2 {
                let refused = register(&emulator, &target).await;
                assert_workspace_not_authorized(
                    &format!("{label} attempt {attempt}"),
                    &target,
                    &refused,
                );
            }

            // Nothing was registered: the registry is exactly what the management
            // account's registrations left, so no record of the refused workspace and no
            // organization-wide grant for it.
            let after = list_workspaces(&emulator, None).await;
            assert_eq!(
                after.body, before.body,
                "case {label:?}: a refused registration must change nothing; the listing \
                 reported {} after having reported {}",
                after.body, before.body
            );
            assert_eq!(
                listed_workspace(&emulator, &target).await,
                None,
                "case {label:?}: the refused workspace must not be in the registry, got {}",
                after.body
            );
            let mut expected: Vec<String> = all.iter().take(already).cloned().collect();
            expected.sort();
            assert_eq!(
                reported_team_ids(&reported(&after)),
                expected,
                "case {label:?}: expected only the workspaces the management account \
                 registered, got {}",
                after.body
            );

            // And the workspace really is unregistered: a channel configuration in it
            // still hits its own missing-workspace precondition.
            let channel = create_channel(&emulator, &target).await;
            assert_eq!(
                channel.status, 409,
                "case {label:?}: expected the channel create to still hit the \
                 missing-workspace precondition, got {} / {}",
                channel.status, channel.body
            );
        }
    }
}

#[tokio::test]
async fn a_member_account_registers_a_workspace_the_management_account_registered() {
    // Rows: every `teamId` of the table, each registered by the management account first,
    // then registered again once the caller is only a member of the organization.
    for (index, team_id) in team_ids().into_iter().enumerate() {
        let master = other_management_accounts()[index % other_management_accounts().len()].clone();
        let label = format!(
            "managed by {master}, team {}",
            &team_id[..team_id.len().min(16)]
        );
        let emulator = Emulator::new();

        let registered = emulator.register_workspace(&team_id).await;
        assert_eq!(
            registered.status, 200,
            "case {label:?}: the management account's registration must answer HTTP 200, \
             got {} / {}",
            registered.status, registered.body
        );
        assert_eq!(
            registered.string("accountType").as_deref(),
            Some(ACCOUNT_TYPE_MANAGEMENT),
            "case {label:?}: expected accountType {ACCOUNT_TYPE_MANAGEMENT:?}, got {}",
            registered.body
        );
        // The management account's registration is what grants member accounts the right
        // to authorize the workspace, so that is what the stored record carries.
        assert_eq!(
            listed_workspace(&emulator, &team_id)
                .await
                .and_then(|record| record.get("allowOrganizationMemberAccount").cloned()),
            Some(json!(true)),
            "case {label:?}: expected the management account's registration to record \
             allowOrganizationMemberAccount true"
        );

        organization_managed_by(&emulator, &label, &master).await;
        let before = list_workspaces(&emulator, None).await;

        // Twice: the same request from a member account is served every time, and it
        // authorizes the workspace for that account alone, so it leaves the stored record
        // alone.
        for attempt in 1..=2 {
            let served = register(&emulator, &team_id).await;
            assert_eq!(
                served.status, 200,
                "case {label:?} attempt {attempt}: expected HTTP 200 for a workspace the \
                 organization has registered, got {} / {}",
                served.status, served.body
            );
            assert_eq!(
                served.string("accountType").as_deref(),
                Some(ACCOUNT_TYPE_MEMBER),
                "case {label:?} attempt {attempt}: the organization is managed by \
                 {master}, so expected accountType {ACCOUNT_TYPE_MEMBER:?}, got {}",
                served.body
            );
            assert_eq!(
                served.string("teamId").as_deref(),
                Some(team_id.as_str()),
                "case {label:?} attempt {attempt}: expected the registered teamId, got {}",
                served.body
            );
            assert_eq!(
                served.string("teamName").as_deref(),
                Some(derived_slack_workspace_name(&team_id).as_str()),
                "case {label:?} attempt {attempt}: expected the recorded teamName, got {}",
                served.body
            );
        }

        let after = list_workspaces(&emulator, None).await;
        assert_eq!(
            after.body, before.body,
            "case {label:?}: a member account's registration must leave the stored record \
             as the management account's left it; the listing reported {} after having \
             reported {}",
            after.body, before.body
        );
    }
}

#[tokio::test]
async fn the_caller_role_is_read_per_call() {
    // The role is not a property of the registry or of the first call: the same request
    // is refused while the caller is a member of the organization and served once the
    // caller manages it again, and only then is the organization-wide grant recorded.
    let caller = engine::emulator_account_id();
    for master in other_management_accounts() {
        let team_id = "T012ABCDEFG";
        let label = format!("managed by {master}");
        let emulator = Emulator::new();
        emulator.ensure_organization().await;

        organization_managed_by(&emulator, &label, &master).await;
        assert_workspace_not_authorized(&label, team_id, &register(&emulator, team_id).await);

        // Handed back: the caller manages the organization again, so the very same
        // request is served and records the grant.
        organization_managed_by(&emulator, &label, caller).await;
        let served = register(&emulator, team_id).await;
        assert_eq!(
            served.status, 200,
            "case {label:?}: expected the management account's registration to be served, \
             got {} / {}",
            served.status, served.body
        );
        assert_eq!(
            served.string("accountType").as_deref(),
            Some(ACCOUNT_TYPE_MANAGEMENT),
            "case {label:?}: expected accountType {ACCOUNT_TYPE_MANAGEMENT:?}, got {}",
            served.body
        );
        assert_eq!(
            listed_workspace(&emulator, team_id)
                .await
                .and_then(|record| record.get("allowOrganizationMemberAccount").cloned()),
            Some(json!(true)),
            "case {label:?}: expected the management account's registration to record \
             allowOrganizationMemberAccount true"
        );

        // And a member account's call for the now registered workspace is served, which
        // is the two-step authorization the operation documents.
        organization_managed_by(&emulator, &label, &master).await;
        let served = register(&emulator, team_id).await;
        assert_eq!(
            served.string("accountType").as_deref(),
            Some(ACCOUNT_TYPE_MEMBER),
            "case {label:?}: expected accountType {ACCOUNT_TYPE_MEMBER:?} once the \
             organization is managed by {master}, got {}",
            served.body
        );
    }
}
