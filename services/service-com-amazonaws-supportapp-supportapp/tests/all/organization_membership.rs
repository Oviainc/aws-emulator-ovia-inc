// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the organization-membership precondition of
//! `RegisterSlackWorkspaceForOrganization`.
//!
//! The rule under test: registering a Slack workspace for the organization is an
//! Organizations-scoped call — "To call this API, your account must be part of an
//! organization in Organizations"
//! (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`) — so an
//! account that belongs to no organization is refused with the modelled
//! `ConflictException` (HTTP 409, the first condition that error enumerates for this
//! operation) and nothing is registered; once an organization holds the account the same
//! request is served, and `accountType` reports the role that organization's own record
//! gives the account rather than a fixed value.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]: the
//! organization is arranged by driving the Organizations emulator's OWN
//! `CreateOrganization` on the same engine, so nothing here reaches into a state struct
//! and a response that is not computed from persisted state cannot pass.
//!
//! The tests are tables. Their rows differ along what the rule generalises over: the
//! `teamId` form and length (the spread [`crate::slack_workspace_configurations`]
//! registers, covering both published `len 1..256` bounds), how many workspaces a call is
//! the first or the second of, and which feature set the organization was created with —
//! the precondition is membership of an organization, not of a particular kind of one.
//! The recorded `T012ABCDEFG` case is one row of many.

use serde_json::{json, Value};
use service_com_amazonaws_organizations_awsorganizationsv20161128::ops::types::ORGANIZATION_FEATURE_SET_VALUES;
use service_com_amazonaws_supportapp_supportapp::ops::types::ACCOUNT_TYPE_VALUES;
use service_com_amazonaws_supportapp_supportapp::state::{
    derived_slack_workspace_name, ACCOUNT_TYPE_MANAGEMENT, ACCOUNT_TYPE_MEMBER,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};
use crate::slack_workspace_configurations::{
    list_workspaces, reported, reported_team_ids, team_ids,
};

/// The operation under test, as its `ConflictException` message names it.
const OPERATION: &str = "RegisterSlackWorkspaceForOrganization";

/// The sentence the operation's own documentation states the precondition in, which its
/// refusal restates
/// (`smithy_query(query="doc",shape="RegisterSlackWorkspaceForOrganization")`).
const DOCUMENTED_RULE: &str = "must be part of an organization in Organizations";

/// Register `team_id` WITHOUT arranging any organization first — the raw call, so the
/// answer is about the account's membership and nothing else. (The harness's own
/// `register_workspace` puts the account in an organization first, which is what every
/// other suite needs.)
async fn register(emulator: &Emulator, team_id: &str) -> Reply {
    emulator
        .call(
            "register-slack-workspace-for-organization",
            json!({ "teamId": team_id }),
        )
        .await
}

/// A create in the workspace `team_id` identifies, which is only possible once that
/// workspace is registered.
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

/// Assert `reply` is the modelled `ConflictException` for an account in no organization:
/// HTTP 409, the error type on the wire, and a message that names the account the
/// emulator presents and restates the operation's documented precondition.
fn assert_not_in_an_organization(label: &str, reply: &Reply) {
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
        message.contains(account) && message.contains(OPERATION) && message.contains(DOCUMENTED_RULE),
        "case {label:?}: expected a message naming account {account}, the {OPERATION} API \
         and the rule {DOCUMENTED_RULE:?}, got {message:?}"
    );
}

/// The `accountType` a successful registration reported, asserted to be one of the
/// values the model declares.
fn reported_account_type(label: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} / {}",
        reply.status, reply.body
    );
    let account_type = reply
        .string("accountType")
        .unwrap_or_else(|| panic!("case {label:?}: expected an accountType, got {}", reply.body));
    assert!(
        ACCOUNT_TYPE_VALUES.contains(&account_type.as_str()),
        "case {label:?}: expected an accountType the model declares, one of \
         {ACCOUNT_TYPE_VALUES:?}, got {account_type:?}"
    );
    account_type
}

#[tokio::test]
async fn registering_a_workspace_from_an_account_in_no_organization_is_refused() {
    for team_id in team_ids() {
        let label = format!("team {}", &team_id[..team_id.len().min(16)]);
        // A fresh emulator: `CreateOrganization` was never called, so this account
        // belongs to no organization.
        let emulator = Emulator::new();

        // Twice, because a refusal must not be a state transition either: the second
        // call is refused for the same reason as the first.
        for attempt in 1..=2 {
            let refused = register(&emulator, &team_id).await;
            assert_not_in_an_organization(&format!("{label} attempt {attempt}"), &refused);
        }

        // Nothing was registered, so the registry is empty.
        let listed = reported(&list_workspaces(&emulator, None).await);
        assert_eq!(
            listed,
            Vec::<Value>::new(),
            "case {label:?}: a refused registration must register nothing, got {listed:?}"
        );

        // And a channel configuration in that workspace still hits its own
        // missing-workspace precondition, which is a different conflict: the workspace
        // was never registered.
        let channel = create_channel(&emulator, &team_id).await;
        assert_eq!(
            channel.status, 409,
            "case {label:?}: expected the channel create to still hit the missing-workspace \
             precondition, got {} / {}",
            channel.status, channel.body
        );

        // Putting the account in an organization is what makes the very same request
        // servable: the precondition is read per call, from the Organizations store.
        let created = emulator.create_organization(None).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the organization must be created, got {} / {}",
            created.status, created.body
        );
        let registered = register(&emulator, &team_id).await;
        let account_type = reported_account_type(&label, &registered);
        assert_eq!(
            registered.string("teamId").as_deref(),
            Some(team_id.as_str()),
            "case {label:?}: expected the registered teamId, got {}",
            registered.body
        );
        assert_eq!(
            registered.string("teamName").as_deref(),
            Some(derived_slack_workspace_name(&team_id).as_str()),
            "case {label:?}: expected the recorded teamName, got {}",
            registered.body
        );

        // The registration is readable back through the listing, and the channel that
        // had nothing to live in is now accepted.
        let listed = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));
        assert_eq!(
            listed,
            vec![team_id.clone()],
            "case {label:?}: expected the registry to hold exactly the registered \
             workspace, got {listed:?}"
        );
        let channel = create_channel(&emulator, &team_id).await;
        assert_eq!(
            channel.status, 200,
            "case {label:?}: expected a channel in the registered workspace to be accepted, \
             got {} / {}",
            channel.status, channel.body
        );

        // The reported role is the one the organization's own record gives this account:
        // `DescribeOrganization` names it as the organization's management account.
        let described = emulator.organizations("DescribeOrganization", json!({})).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: DescribeOrganization must answer HTTP 200, got {} / {}",
            described.status, described.body
        );
        let management = described.body["Organization"]["MasterAccountId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = if management == engine::emulator_account_id() {
            ACCOUNT_TYPE_MANAGEMENT
        } else {
            ACCOUNT_TYPE_MEMBER
        };
        assert_eq!(
            account_type, expected,
            "case {label:?}: the caller is account {}, the organization is managed by \
             {management:?}, so expected accountType {expected:?}, got {account_type:?}",
            engine::emulator_account_id()
        );
    }
}

#[tokio::test]
async fn any_organization_satisfies_the_precondition_whatever_its_feature_set() {
    // The precondition is membership of an organization, so it holds for an organization
    // of every kind the Organizations model declares: rows are `CreateOrganization` with
    // no `FeatureSet` (its documented default) and with each value of
    // `OrganizationFeatureSet`
    // (`smithy_query(query="shape",shape="OrganizationFeatureSet")`, re-exported by that
    // service as `ORGANIZATION_FEATURE_SET_VALUES`).
    let mut feature_sets: Vec<Option<&str>> = vec![None];
    feature_sets.extend(ORGANIZATION_FEATURE_SET_VALUES.iter().map(|value| Some(*value)));
    assert!(
        feature_sets.len() >= 3,
        "expected the model's feature sets plus the default, got {feature_sets:?}"
    );

    for feature_set in feature_sets {
        let label = format!("feature set {feature_set:?}");
        let emulator = Emulator::new();

        // Before the organization exists the call is refused, whichever kind of
        // organization is about to be created.
        assert_not_in_an_organization(&label, &register(&emulator, "T012ABCDEFG").await);

        let created = emulator.create_organization(feature_set).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the organization must be created, got {} / {}",
            created.status, created.body
        );

        // Every row of the team-id table registers once the account is in an
        // organization, and each reports the role the organization gives the account.
        for team_id in team_ids() {
            let row = format!("{label}, team {}", &team_id[..team_id.len().min(16)]);
            let registered = register(&emulator, &team_id).await;
            let account_type = reported_account_type(&row, &registered);
            assert_eq!(
                account_type, ACCOUNT_TYPE_MANAGEMENT,
                "case {row:?}: the account that created the organization manages it, so \
                 expected accountType {ACCOUNT_TYPE_MANAGEMENT:?}, got {account_type:?}"
            );
        }

        let listed = reported_team_ids(&reported(&list_workspaces(&emulator, None).await));
        let mut expected = team_ids();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {label:?}: expected the registry to hold every registered workspace, \
             got {listed:?}"
        );
    }
}
