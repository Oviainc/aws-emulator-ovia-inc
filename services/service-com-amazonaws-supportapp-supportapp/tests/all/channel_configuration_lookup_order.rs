// SPDX-License-Identifier: CC0-1.0

//! Which refusal wins when a Slack channel configuration write names a configuration
//! the account does not hold.
//!
//! An operation that acts on the resource its request IDENTIFIES settles that
//! resource's existence against the account's state before any account-level
//! precondition: `UpdateSlackChannelConfiguration` and
//! `DeleteSlackChannelConfiguration` name one Slack channel configuration by its
//! `(teamId, channelId)` pair, so a pair the account holds no configuration for is the
//! modelled `ResourceNotFoundException` (HTTP 404) — "The specified resource is missing
//! or doesn't exist, such as an account alias, Slack channel configuration, or Slack
//! workspace configuration"
//! (`smithy_query(query="operation", operation="UpdateSlackChannelConfiguration,DeleteSlackChannelConfiguration",
//! model_path="services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json")`)
//! — with the message template every operation of this service words a miss with, and
//! never the modelled `ConflictException` (409) of an account that has registered no
//! Slack workspace.
//!
//! So the tables below run the rule over what it generalises over, with the recorded
//! "no workspace registered at all" case as a single row:
//!
//! * which operation is called — both that act on an existing configuration;
//! * what the account holds ABOUT the named workspace — nothing, a registration, or a
//!   registration it then deleted (which took that workspace's configurations with it);
//! * how much the account holds BESIDES it — no other workspace, one, or a registry
//!   filled to the documented workspace quota, each with its own configuration stored;
//! * the form and length of the `teamId` and `channelId` the request names, at both
//!   published `len 1..256` bounds;
//! * which optional members an update carries, since the answer is about the resource
//!   and not about the patch.
//!
//! The create path keeps the conflict, because the configuration it names cannot
//! already exist: that side of the rule is [`crate::channel_configuration_workspace_precondition`].
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`], so an answer
//! that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::MAX_SLACK_WORKSPACE_CONFIGURATIONS;

use crate::channel_configuration_workspace_precondition::{create_body, update_body};
use crate::error_message_templates::{not_found, refusal};
use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// `teamId` and `channelId` are `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="UpdateSlackChannelConfiguration")`).
const SLACK_ID_MAX_LEN: usize = 256;

/// What the account holds about the workspace the request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workspace {
    /// The account has never registered it — the recorded case.
    NotRegistered,
    /// It is registered, and the channel the request names is not configured in it.
    Registered,
    /// It was registered and the channel configured in it, and the account then deleted
    /// the workspace, which took that configuration with it.
    RegisteredThenRemoved,
}

/// One row: a `(teamId, channelId)` pair the account holds no configuration for, and
/// the account state it is looked up in.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    team_id: String,
    channel_id: String,
    workspace: Workspace,
    /// How many OTHER workspaces the account has registered, each holding a
    /// configuration of its own, so no row's answer can come from an empty account.
    other_workspaces: usize,
}

/// A team id for the `index`th other workspace. Prefixed so it can never collide with a
/// `team_id` a row names.
fn other_team_id(index: usize) -> String {
    format!("TNEIGHBOUR{index}")
}

/// The channel configured in the `index`th other workspace.
fn other_channel_id(index: usize) -> String {
    format!("CNEIGHBOUR{index}")
}

/// The table. Rows differ in the account state the lookup happens in, in how much the
/// account holds besides the named workspace, and in the form and length of the ids.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no workspace registered at all",
            team_id: "T99999ZZZZZ".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            workspace: Workspace::NotRegistered,
            other_workspaces: 0,
        },
        Case {
            label: "no such workspace, one other workspace registered",
            team_id: "T33A749D3D8".to_string(),
            channel_id: "C-other".to_string(),
            workspace: Workspace::NotRegistered,
            other_workspaces: 1,
        },
        Case {
            label: "no such workspace, the registry filled to the documented quota",
            team_id: "T-mixed.Case_9".to_string(),
            channel_id: "C#9/x".to_string(),
            workspace: Workspace::NotRegistered,
            other_workspaces: MAX_SLACK_WORKSPACE_CONFIGURATIONS,
        },
        Case {
            label: "the shortest legal ids, nothing registered",
            team_id: "T".to_string(),
            channel_id: "C".to_string(),
            workspace: Workspace::NotRegistered,
            other_workspaces: 0,
        },
        Case {
            label: "the longest legal ids, nothing registered",
            team_id: "T".repeat(SLACK_ID_MAX_LEN),
            channel_id: "C".repeat(SLACK_ID_MAX_LEN),
            workspace: Workspace::NotRegistered,
            other_workspaces: 0,
        },
        Case {
            label: "the workspace is registered, the channel is not configured",
            team_id: "T012ABCDEFG".to_string(),
            channel_id: "C99999ZZZZZ".to_string(),
            workspace: Workspace::Registered,
            other_workspaces: 1,
        },
        Case {
            label: "the longest legal ids, the workspace registered",
            team_id: "T".repeat(SLACK_ID_MAX_LEN),
            channel_id: "C".repeat(SLACK_ID_MAX_LEN),
            workspace: Workspace::Registered,
            other_workspaces: MAX_SLACK_WORKSPACE_CONFIGURATIONS - 1,
        },
        Case {
            label: "the configuration went with its deleted workspace",
            team_id: "T0123456789".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            workspace: Workspace::RegisteredThenRemoved,
            other_workspaces: 1,
        },
        Case {
            label: "the shortest legal ids, the workspace deleted again",
            team_id: "t".to_string(),
            channel_id: "c".to_string(),
            workspace: Workspace::RegisteredThenRemoved,
            other_workspaces: 0,
        },
    ]
}

/// The updates one row is driven with: the answer is about the resource the request
/// names, so it cannot depend on which optional members the patch carries.
fn update_bodies(team_id: &str, channel_id: &str) -> Vec<(&'static str, Value)> {
    let mut with_everything = update_body(team_id, channel_id);
    with_everything["notifyOnCaseSeverity"] = json!("all");
    with_everything["notifyOnResolveCase"] = json!(true);
    with_everything["notifyOnCreateOrReopenCase"] = json!(true);
    with_everything["notifyOnAddCorrespondenceToCase"] = json!(true);
    with_everything["channelName"] = json!("aws-support");
    with_everything["channelRoleArn"] = json!(role_arn("SupportAppRole"));

    vec![
        (
            "only the required members",
            json!({ "teamId": team_id, "channelId": channel_id }),
        ),
        ("a severity only", update_body(team_id, channel_id)),
        ("every optional member", with_everything),
    ]
}

/// Build the account state `case` describes, and leave it holding no configuration for
/// the pair the row names.
async fn arrange(case: &Case) -> Emulator {
    let emulator = Emulator::new();

    for index in 0..case.other_workspaces {
        let team_id = other_team_id(index);
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {:?}: the other workspace {team_id:?} must be registered",
            case.label
        );
        assert_eq!(
            emulator
                .create(create_body(&team_id, &other_channel_id(index)))
                .await
                .status,
            200,
            "case {:?}: the other workspace's configuration must be created",
            case.label
        );
    }

    match case.workspace {
        Workspace::NotRegistered => {}
        Workspace::Registered => {
            assert_eq!(
                emulator.register_workspace(&case.team_id).await.status,
                200,
                "case {:?}: the named workspace must be registered",
                case.label
            );
        }
        Workspace::RegisteredThenRemoved => {
            assert_eq!(
                emulator.register_workspace(&case.team_id).await.status,
                200,
                "case {:?}: the named workspace must be registered",
                case.label
            );
            assert_eq!(
                emulator
                    .create(create_body(&case.team_id, &case.channel_id))
                    .await
                    .status,
                200,
                "case {:?}: the configuration must be created before its workspace goes",
                case.label
            );
            assert_eq!(
                emulator.delete_workspace(&case.team_id).await.status,
                200,
                "case {:?}: the named workspace must be removable",
                case.label
            );
        }
    }

    emulator
}

/// Assert `reply` is the modelled miss for `(team_id, channel_id)`: HTTP 404, the error
/// type on the wire, and the message template this service words a miss with.
fn assert_miss(case: &str, team_id: &str, channel_id: &str, reply: &Reply) {
    let message = refusal(case, reply, 404, "ResourceNotFoundException");
    let expected = not_found(team_id, channel_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// Assert the account still holds exactly the other workspaces' configurations and
/// nothing for the pair the row named: a refused write writes nothing.
async fn assert_store_untouched(emulator: &Emulator, case: &Case) {
    let label = case.label;
    assert!(
        emulator
            .listed(&case.team_id, &case.channel_id)
            .await
            .is_none(),
        "case {label:?}: a refused write must not create the configuration it refused, got {}",
        emulator.list(None).await.body
    );
    for index in 0..case.other_workspaces {
        assert!(
            emulator
                .listed(&other_team_id(index), &other_channel_id(index))
                .await
                .is_some(),
            "case {label:?}: the other workspace's configuration must survive, got {}",
            emulator.list(None).await.body
        );
    }
    assert_eq!(
        emulator.list(None).await.configurations().len(),
        case.other_workspaces,
        "case {label:?}: expected exactly {} stored configurations, got {}",
        case.other_workspaces,
        emulator.list(None).await.body
    );
}

#[tokio::test]
async fn deleting_a_configuration_the_account_does_not_hold_is_the_miss() {
    for case in cases() {
        let emulator = arrange(&case).await;

        // Twice: the refusal changed nothing, so the second call answers the same.
        for attempt in 1..=2 {
            let reply = emulator.delete(&case.team_id, &case.channel_id).await;
            assert_miss(
                &format!("{} / delete, attempt {attempt}", case.label),
                &case.team_id,
                &case.channel_id,
                &reply,
            );
        }

        assert_store_untouched(&emulator, &case).await;
    }
}

#[tokio::test]
async fn updating_a_configuration_the_account_does_not_hold_is_the_miss() {
    for case in cases() {
        let emulator = arrange(&case).await;

        for (patch, body) in update_bodies(&case.team_id, &case.channel_id) {
            let reply = emulator.update(body).await;
            assert_miss(
                &format!("{} / update with {patch}", case.label),
                &case.team_id,
                &case.channel_id,
                &reply,
            );
        }

        assert_store_untouched(&emulator, &case).await;
    }
}

#[tokio::test]
async fn the_miss_is_read_from_the_store_not_assumed() {
    // The very same calls on the very same ids answer 200 once the configuration the
    // request names IS stored — so the 404 the tables above assert is computed from the
    // account's state and not from the request. Rows reuse the table's id forms.
    for case in cases() {
        let label = case.label;
        let emulator = Emulator::new();

        // Registering the workspace and creating the pair is what makes the
        // configuration exist.
        assert_eq!(
            emulator.register_workspace(&case.team_id).await.status,
            200,
            "case {label:?}: the workspace must be registrable"
        );
        assert_eq!(
            emulator
                .create(create_body(&case.team_id, &case.channel_id))
                .await
                .status,
            200,
            "case {label:?}: the configuration must be creatable"
        );

        let updated = emulator
            .update(update_body(&case.team_id, &case.channel_id))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected the update of the stored configuration to answer \
             HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        let deleted = emulator.delete(&case.team_id, &case.channel_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected the delete of the stored configuration to answer \
             HTTP 200, got {} / {}",
            deleted.status, deleted.body
        );

        // And it is the miss again once it is gone.
        assert_miss(
            &format!("{label} / delete after the delete"),
            &case.team_id,
            &case.channel_id,
            &emulator.delete(&case.team_id, &case.channel_id).await,
        );
        assert_eq!(
            emulator.list(None).await.configurations(),
            Vec::<Value>::new(),
            "case {label:?}: expected the account to hold no configuration after the \
             delete, got {}",
            emulator.list(None).await.body
        );
    }
}
