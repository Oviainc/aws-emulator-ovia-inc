// SPDX-License-Identifier: CC0-1.0

//! The workspace precondition a Slack channel configuration CREATE enforces.
//!
//! A Slack channel configuration lives inside a Slack workspace registered to the
//! account, so a request that ADDS one in a workspace this account has not registered
//! is refused with the modelled `ConflictException` (409) — the condition
//! `ConflictException`'s own documentation names first: "Add, update, or delete a Slack
//! channel configuration before you add a Slack workspace to your Amazon Web Services
//! account" (`smithy_query(query="doc", shape="ConflictException")`).
//!
//! The operations that act on an EXISTING configuration — `Update` and `Delete` — settle
//! that configuration's existence first, so a pair the account holds nothing for is the
//! modelled `ResourceNotFoundException` (404) whether or not the workspace is
//! registered; that ordering is the subject of
//! [`crate::channel_configuration_lookup_order`], and the rows here pin the two answers
//! side by side.
//!
//! The tests are tables whose rows differ along what the rule generalises over: which
//! of the three writing operations is called (`CreateSlackChannelConfiguration`,
//! `UpdateSlackChannelConfiguration`, `DeleteSlackChannelConfiguration` — every
//! operation whose model binds `ConflictException` and that writes a channel
//! configuration), the form and length of the `teamId` (both published `len 1..256`
//! bounds), and what the account's registry holds: nothing, another workspace, or the
//! requested workspace.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`], so a status
//! or a message that is not computed from the persisted registry cannot pass.

use serde_json::{json, Value};

use crate::error_message_templates::{not_found, refusal};
use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// `teamId` is `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="DeleteSlackChannelConfiguration")`), so
/// the rows below cover both published bounds.
const SLACK_ID_MAX_LEN: usize = 256;

/// The message the unregistered-workspace conflict carries: the condition stated over
/// the account the request was made in and the `teamId` it named.
fn workspace_conflict(team_id: &str) -> String {
    format!(
        "Workspace installation not found for account {account} and team {team_id}",
        account = engine::emulator_account_id()
    )
}

/// Assert `reply` is the modelled `ConflictException` for an unregistered workspace:
/// HTTP 409, the error type on the wire, and the message built from the request.
fn assert_workspace_conflict(case: &str, team_id: &str, reply: &Reply) {
    let message = refusal(case, reply, 409, "ConflictException");
    let expected = workspace_conflict(team_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// Assert `reply` is the modelled `ResourceNotFoundException` for a channel that is not
/// configured, which is what the operations acting on an existing configuration answer.
fn assert_channel_not_found(case: &str, team_id: &str, channel_id: &str, reply: &Reply) {
    let message = refusal(case, reply, 404, "ResourceNotFoundException");
    let expected = not_found(team_id, channel_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// A create request for `(team_id, channel_id)` that satisfies every modelled
/// constraint and the documented severity rule, so the only thing left that can refuse
/// it is an account-state rule.
pub(crate) fn create_body(team_id: &str, channel_id: &str) -> Value {
    json!({
        "teamId": team_id,
        "channelId": channel_id,
        "notifyOnCaseSeverity": "none",
        "channelRoleArn": role_arn("SupportAppRole"),
    })
}

/// An update request for `(team_id, channel_id)` that satisfies every modelled
/// constraint and the documented severity rule.
pub(crate) fn update_body(team_id: &str, channel_id: &str) -> Value {
    json!({
        "teamId": team_id,
        "channelId": channel_id,
        "notifyOnCaseSeverity": "none",
    })
}

/// Assert what each of the three channel-configuration writes answers for a
/// `(team_id, channel_id)` pair that is NOT configured and whose workspace the account
/// has not registered: the create has no workspace to add a configuration to, so it is
/// the documented conflict, while the update and the delete name a configuration that
/// does not exist, which is the miss.
async fn assert_writes_on_an_unregistered_workspace(
    emulator: &Emulator,
    label: &str,
    team_id: &str,
    channel_id: &str,
) {
    assert_workspace_conflict(
        &format!("{label} / CreateSlackChannelConfiguration"),
        team_id,
        &emulator.create(create_body(team_id, channel_id)).await,
    );
    assert_channel_not_found(
        &format!("{label} / UpdateSlackChannelConfiguration"),
        team_id,
        channel_id,
        &emulator.update(update_body(team_id, channel_id)).await,
    );
    assert_channel_not_found(
        &format!("{label} / DeleteSlackChannelConfiguration"),
        team_id,
        channel_id,
        &emulator.delete(team_id, channel_id).await,
    );
}

/// The team ids the tables run over: the recorded one, both published `@length` bounds,
/// a punctuated id and an all-digit one.
pub(crate) fn team_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the recorded team id", "T99999ZZZZZ".to_string()),
        ("the shortest legal team id", "T".to_string()),
        ("a punctuated mixed-case team id", "T-mixed.Case_9".to_string()),
        ("the longest legal team id", "T".repeat(SLACK_ID_MAX_LEN)),
        ("an all-digit team id", "0123456789".to_string()),
    ]
}

#[tokio::test]
async fn adding_a_channel_configuration_needs_a_registered_workspace() {
    for (label, team_id) in team_ids() {
        let emulator = Emulator::new();
        let channel_id = "C01234A5BCD";

        // Nothing is registered, so the create has no workspace to add a configuration
        // to, and the two writes that act on an existing one report the miss.
        assert_writes_on_an_unregistered_workspace(
            &emulator,
            &format!("{label}, empty registry"),
            &team_id,
            channel_id,
        )
        .await;
        // And none of them wrote anything.
        assert_eq!(
            emulator.list(None).await.configurations(),
            Vec::<Value>::new(),
            "case {label:?}: a refused write must store nothing"
        );

        // Registering the workspace the requests name is what satisfies the
        // create's precondition: the create is then accepted, while the two writes that
        // need a configuration keep reporting the MISS for a channel that is not
        // configured.
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );
        let absent = "C99999ZZZZZ";
        assert_channel_not_found(
            &format!("{label} / UpdateSlackChannelConfiguration, workspace registered"),
            &team_id,
            absent,
            &emulator.update(update_body(&team_id, absent)).await,
        );
        assert_channel_not_found(
            &format!("{label} / DeleteSlackChannelConfiguration, workspace registered"),
            &team_id,
            absent,
            &emulator.delete(&team_id, absent).await,
        );

        let created = emulator.create(create_body(&team_id, channel_id)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the create to succeed once the workspace was \
             registered, got {} / {}",
            created.status, created.body
        );
        let updated = emulator.update(update_body(&team_id, channel_id)).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected the update of the created configuration to \
             succeed, got {} / {}",
            updated.status, updated.body
        );
        let deleted = emulator.delete(&team_id, channel_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected the delete of the created configuration to \
             succeed, got {} / {}",
            deleted.status, deleted.body
        );

        // The configuration is gone, so deleting the same pair again is the miss.
        assert_channel_not_found(
            &format!("{label} / second DeleteSlackChannelConfiguration"),
            &team_id,
            channel_id,
            &emulator.delete(&team_id, channel_id).await,
        );
    }
}

#[tokio::test]
async fn a_workspace_registered_for_another_team_does_not_satisfy_the_precondition() {
    // The precondition is per `teamId`: a registry holding some OTHER workspace does not
    // let a channel configuration be added to the one the request names. Rows differ
    // in which workspace is registered and which one the write asks for, including a
    // prefix of the requested id and both published length bounds.
    let rows: [(&str, String, String); 4] = [
        (
            "an unrelated workspace is registered",
            "T012ABCDEFG".to_string(),
            "T99999ZZZZZ".to_string(),
        ),
        (
            "the registered id is a prefix of the requested one",
            "T012ABCDEFG".to_string(),
            "T012ABCDEFGH".to_string(),
        ),
        (
            "the shortest legal id is registered",
            "T".to_string(),
            "t".to_string(),
        ),
        (
            "the longest legal id is registered",
            "T".repeat(SLACK_ID_MAX_LEN),
            "T".repeat(SLACK_ID_MAX_LEN - 1),
        ),
    ];

    for (label, registered, requested) in rows {
        let emulator = Emulator::new();
        let channel_id = "C01234A5BCD";
        assert_eq!(
            emulator.register_workspace(&registered).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );
        // A configuration in the registered workspace, so the refusals below are about
        // the workspace the request named and not about an empty store.
        assert_eq!(
            emulator
                .create(create_body(&registered, channel_id))
                .await
                .status,
            200,
            "case {label:?}: the configuration in the registered workspace must be created"
        );

        assert_writes_on_an_unregistered_workspace(&emulator, label, &requested, channel_id).await;

        // The configuration in the registered workspace is untouched, and its own
        // writes still work.
        assert!(
            emulator.listed(&registered, channel_id).await.is_some(),
            "case {label:?}: the configuration in the registered workspace must survive, got {}",
            emulator.list(None).await.body
        );
        assert_eq!(
            emulator
                .update(update_body(&registered, channel_id))
                .await
                .status,
            200,
            "case {label:?}: expected the update in the registered workspace to succeed"
        );
        assert_eq!(
            emulator.delete(&registered, channel_id).await.status,
            200,
            "case {label:?}: expected the delete in the registered workspace to succeed"
        );
    }
}

#[tokio::test]
async fn removing_the_workspace_makes_adding_a_channel_the_conflict_again() {
    // The precondition is read from the registry at every create, so a workspace the
    // account REMOVES takes the precondition with it: adding a channel in it is the
    // conflict again, whatever was configured in it before — and because the workspace
    // delete took its configurations with it, the update and the delete of each of them
    // report the miss. Rows differ in the team id form and in how many channels the
    // workspace held.
    let rows: [(&str, String, &[&str]); 3] = [
        ("one channel", "T012ABCDEFG".to_string(), &["C01234A5BCD"]),
        (
            "three channels",
            "T-mixed.Case_9".to_string(),
            &["C001", "C002", "C003"],
        ),
        (
            "the longest legal team id, one channel",
            "T".repeat(SLACK_ID_MAX_LEN),
            &["C"],
        ),
    ];

    for (label, team_id, channels) in rows {
        let emulator = Emulator::new();
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );
        for channel_id in channels {
            assert_eq!(
                emulator
                    .create(create_body(&team_id, channel_id))
                    .await
                    .status,
                200,
                "case {label:?}: the configuration for {channel_id:?} must be created"
            );
        }

        assert_eq!(
            emulator.delete_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be removable"
        );

        for channel_id in channels {
            assert_writes_on_an_unregistered_workspace(
                &emulator,
                &format!("{label} / {channel_id} after the workspace went"),
                &team_id,
                channel_id,
            )
            .await;
        }

        // Registering the workspace again brings back the precondition, not the
        // configurations that lived in it: each channel is then the miss.
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registrable again"
        );
        for channel_id in channels {
            assert_channel_not_found(
                &format!("{label} / DeleteSlackChannelConfiguration on {channel_id} after re-registering"),
                &team_id,
                channel_id,
                &emulator.delete(&team_id, channel_id).await,
            );
        }
    }
}
