// SPDX-License-Identifier: CC0-1.0

//! The ONE registry-presence rule both readers of the Slack workspace registry decide by.
//!
//! The rule under test: a Slack workspace is registered to this account exactly when the
//! account's workspace registry holds its `teamId`, and every decision that turns on it
//! reads that one fact — `CreateSlackChannelConfiguration`'s documented conflict ("Add,
//! update, or delete a Slack channel configuration before you add a Slack workspace to
//! your Amazon Web Services account", `smithy_query(query="doc", shape="ConflictException")`)
//! and `RegisterSlackWorkspaceForOrganization`'s member-account conflict ("Call the
//! RegisterSlackWorkspaceForOrganization API from a member account, but the management
//! account hasn't registered that workspace yet for the organization", same shape) with
//! the documented workspace quota accounting that rides on it ("You can add up to 5 Slack
//! workspaces for your account",
//! `smithy_query(query="doc", shape="CreateSlackChannelConfiguration")`).
//!
//! So the two operations can never disagree about one `teamId`: for any registry state,
//! either both are served or both answer their own documented conflict, and a change to
//! the registry — a registration, a removal — moves both answers together.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`], so an answer
//! that is not computed from the persisted registry cannot pass. The tests are tables
//! whose rows differ along what the rule generalises over: the `teamId` form and length
//! (the spread [`crate::slack_workspace_configurations`] registers, covering both
//! published `len 1..256` bounds), which account manages the organization, what else the
//! registry holds, and whether the workspace in the request is in it.

use serde_json::Value;
use service_com_amazonaws_supportapp_supportapp::state::{
    ACCOUNT_TYPE_MANAGEMENT, ACCOUNT_TYPE_MEMBER, MAX_SLACK_WORKSPACE_CONFIGURATIONS,
};

use crate::channel_configuration_workspace_precondition::create_body;
use crate::slack_channel_configurations::{Emulator, Reply};
use crate::slack_workspace_configurations::{list_workspaces, reported, team_ids};

/// The channel a row configures; the rule under test is about the workspace, so one
/// channel id is enough.
const CHANNEL_ID: &str = "C01234A5BCD";

/// The management accounts a row hands the organization to, so the caller is a MEMBER of
/// its organization: a 12-digit account id other than the one the emulator presents.
const OTHER_MANAGEMENT_ACCOUNTS: [&str; 2] = ["111122223333", "999988887777"];

/// The message the create path's unregistered-workspace conflict carries.
fn workspace_conflict(team_id: &str) -> String {
    format!(
        "Workspace installation not found for account {account} and team {team_id}",
        account = engine::emulator_account_id()
    )
}

/// Make the calling account a MEMBER of its organization, by creating the organization
/// through the Organizations emulator's own `CreateOrganization` and handing it to
/// `master_account_id`; `None` hands it back to the calling account, which then manages
/// it again.
async fn organization_managed_by(emulator: &Emulator, master_account_id: Option<&str>) {
    emulator.ensure_organization().await;
    let master = master_account_id.unwrap_or_else(|| engine::emulator_account_id());
    emulator.hand_organization_to(master);
}

/// Register `team_id` with whatever role the organization currently gives this account.
/// The workspace is authorized in the Support Center console first, which is the
/// precondition this operation settles ahead of anything about the calling account.
async fn register(emulator: &Emulator, team_id: &str) -> Reply {
    emulator.authorize_workspace_in_console(team_id);
    emulator
        .call(
            "register-slack-workspace-for-organization",
            serde_json::json!({ "teamId": team_id }),
        )
        .await
}

/// Add a Slack channel configuration in `team_id` — the create path's reader of the
/// registry.
async fn create_channel(emulator: &Emulator, team_id: &str) -> Reply {
    emulator.create(create_body(team_id, CHANNEL_ID)).await
}

/// Assert both readers of the registry answer `team_id` as a workspace this account has
/// NOT registered: the create path with its documented conflict, a member account's
/// registration with its own.
async fn assert_both_refuse(emulator: &Emulator, case: &str, team_id: &str) {
    let created = create_channel(emulator, team_id).await;
    assert_eq!(
        created.status, 409,
        "case {case:?}: expected the create path to answer HTTP 409 for an unregistered \
         workspace, got {} / {}",
        created.status, created.body
    );
    assert_eq!(
        created.error_type, "ConflictException",
        "case {case:?}: expected ConflictException from the create path, got {:?}",
        created.error_type
    );
    let expected = workspace_conflict(team_id);
    let message = created.string("message").unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?} from the create path, got {message:?}"
    );

    let registered = register(emulator, team_id).await;
    assert_eq!(
        registered.status, 409,
        "case {case:?}: expected a member account's registration to answer HTTP 409 for a \
         workspace the organization has not registered, got {} / {}",
        registered.status, registered.body
    );
    assert_eq!(
        registered.error_type, "ConflictException",
        "case {case:?}: expected ConflictException from the register path, got {:?}",
        registered.error_type
    );
    let message = registered.string("message").unwrap_or_default();
    assert!(
        message.contains(team_id) && message.contains("member account"),
        "case {case:?}: expected a message naming workspace {team_id} and the member \
         account, got {message:?}"
    );
}

/// Assert both readers of the registry answer `team_id` as a workspace this account HAS
/// registered: a member account's registration is served, and so is a channel
/// configuration in it.
async fn assert_both_served(emulator: &Emulator, case: &str, team_id: &str) {
    let registered = register(emulator, team_id).await;
    assert_eq!(
        registered.status, 200,
        "case {case:?}: expected a member account's registration of a registered \
         workspace to be served, got {} / {}",
        registered.status, registered.body
    );
    assert_eq!(
        registered.string("accountType").as_deref(),
        Some(ACCOUNT_TYPE_MEMBER),
        "case {case:?}: expected accountType {ACCOUNT_TYPE_MEMBER:?}, got {}",
        registered.body
    );

    let created = create_channel(emulator, team_id).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the create path to be served in a registered workspace, \
         got {} / {}",
        created.status, created.body
    );
    assert!(
        emulator.listed(team_id, CHANNEL_ID).await.is_some(),
        "case {case:?}: expected the created configuration to be stored in the \
         registered workspace, got {}",
        emulator.list(None).await.body
    );
}

#[tokio::test]
async fn both_registry_readers_answer_one_team_id_the_same_way() {
    let all = team_ids();
    // Rows: the `teamId` form and length, which account manages the organization, and how
    // many OTHER workspaces the registry already holds — so each answer is about the
    // workspace in the request and not about an empty or a full registry.
    for (index, team_id) in all.iter().enumerate() {
        let master = OTHER_MANAGEMENT_ACCOUNTS[index % OTHER_MANAGEMENT_ACCOUNTS.len()];
        let others: Vec<&str> = all
            .iter()
            .filter(|other| *other != team_id)
            .map(String::as_str)
            .collect();
        let already = index % 3;
        let case = format!(
            "team {}, managed by {master}, {already} other workspaces registered",
            &team_id[..team_id.len().min(16)]
        );

        let emulator = Emulator::new();
        for other in others.iter().take(already) {
            assert_eq!(
                emulator.register_workspace(other).await.status,

                200,
                "case {case:?}: the other workspace {} must be registered by the \
                 management account",
                &other[..other.len().min(16)]
            );
        }
        organization_managed_by(&emulator, Some(master)).await;

        // Not in the registry: both readers refuse, each with its own documented
        // conflict, and nothing is stored.
        assert_both_refuse(&emulator, &format!("{case} / unregistered"), team_id).await;
        assert!(
            !reported_holds(&emulator, team_id).await,
            "case {case:?}: a refused call must register nothing"
        );

        // The management account registers it: the one registry fact flips, so both
        // readers are served.
        organization_managed_by(&emulator, None).await;
        let registered = register(&emulator, team_id).await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: the management account's registration must be served, got {} / {}",
            registered.status, registered.body
        );
        assert_eq!(
            registered.string("accountType").as_deref(),
            Some(ACCOUNT_TYPE_MANAGEMENT),
            "case {case:?}: expected accountType {ACCOUNT_TYPE_MANAGEMENT:?}, got {}",
            registered.body
        );
        organization_managed_by(&emulator, Some(master)).await;
        assert_both_served(&emulator, &format!("{case} / registered"), team_id).await;

        // Removing it flips that one fact back, and both readers refuse again.
        assert_eq!(
            emulator.delete_workspace(team_id).await.status,
            200,
            "case {case:?}: the workspace must be removable"
        );
        assert_both_refuse(&emulator, &format!("{case} / removed"), team_id).await;

        // The workspaces the row registered first are untouched by any of it, so every
        // answer above was about the `teamId` in the request.
        for other in others.iter().take(already) {
            assert!(
                reported_holds(&emulator, other).await,
                "case {case:?}: the other workspace {} must still be registered",
                &other[..other.len().min(16)]
            );
        }
    }
}

/// Whether `ListSlackWorkspaceConfigurations` reports a record for `team_id` — the
/// registry read back over the wire.
async fn reported_holds(emulator: &Emulator, team_id: &str) -> bool {
    reported(&list_workspaces(emulator, None).await)
        .iter()
        .any(|record| record.get("teamId").and_then(Value::as_str) == Some(team_id))
}

#[tokio::test]
async fn the_register_path_counts_against_the_quota_only_what_the_registry_lacks() {
    // The register path's OTHER decision that turns on the same registry fact: a
    // registration of a workspace the registry already holds adds nothing to the account,
    // so it is served even with the registry at the documented quota, while one it lacks
    // is refused as past that quota. The create path reads the same fact: it is served in
    // each held workspace and refused in the one the quota kept out.
    let all = team_ids();
    assert_eq!(
        all.len(),
        MAX_SLACK_WORKSPACE_CONFIGURATIONS,
        "the table must fill exactly the documented workspace quota, got {}",
        all.len()
    );
    let emulator = Emulator::new();
    for team_id in &all {
        assert_eq!(
            emulator.register_workspace(team_id).await.status,
            200,
            "the registry must fill to its quota: {} was refused",
            &team_id[..team_id.len().min(16)]
        );
    }

    // Rows: every workspace the registry holds — each re-registered at the quota and each
    // configured — then a workspace it does not hold.
    for team_id in &all {
        let case = format!("held team {}", &team_id[..team_id.len().min(16)]);
        let again = register(&emulator, team_id).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: re-registering a held workspace at the quota must be served, \
             got {} / {}",
            again.status, again.body
        );
        let created = create_channel(&emulator, team_id).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create path must be served in a held workspace, got {} / {}",
            created.status, created.body
        );
    }

    for absent in ["T777NEWNEWW", "Z"] {
        let case = format!("absent team {absent}");
        let refused = register(&emulator, absent).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a registration past the documented quota must be refused, \
             got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
        assert!(
            !reported_holds(&emulator, absent).await,
            "case {case:?}: the refused registration must store nothing"
        );
        let created = create_channel(&emulator, absent).await;
        let expected = workspace_conflict(absent);
        let message = created.string("message").unwrap_or_default();
        assert_eq!(
            created.status, 409,
            "case {case:?}: expected the create path to refuse the workspace the quota \
             kept out, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?} from the create path, got {message:?}"
        );
    }
}
