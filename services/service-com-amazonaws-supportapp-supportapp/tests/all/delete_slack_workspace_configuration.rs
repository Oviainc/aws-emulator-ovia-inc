// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteSlackWorkspaceConfiguration`: what removing a Slack
//! workspace configuration from an account changes.
//!
//! The rule under test is that `teamId` identifies a Slack workspace configuration in
//! this account's registry, so the delete is a lookup in that registry: a registered
//! `teamId` is removed and anything that was scoped to it goes with it, and a `teamId`
//! the account holds no configuration for — never registered, or already deleted — is
//! the modelled `ResourceNotFoundException` (404).
//!
//! Nothing here reads the service's state struct: every assertion goes over the wire
//! through the same router the production emulator mounts, reusing the harness in
//! [`crate::slack_channel_configurations`] and the workspace listing helpers in
//! [`crate::slack_workspace_configurations`]. The removal is observed through the
//! operations that READ the registry — `ListSlackWorkspaceConfigurations`, the delete
//! itself, and `CreateSlackChannelConfiguration`, whose documented precondition is
//! that the channel's workspace is registered to the account — so a 200 that persisted
//! nothing, or a 404 that is not computed from the registry, cannot pass.
//!
//! The tests are tables whose rows differ along what the rule generalises over: the
//! `teamId` form and length (both published `len 1..256` bounds), how many channel
//! configurations the workspace holds, which other workspaces the account has
//! registered, and how full the registry is. The recorded `T012ABCDEFG` case is one
//! row.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::MAX_SLACK_WORKSPACE_CONFIGURATIONS;

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};
use crate::slack_workspace_configurations::{list_workspaces, reported, reported_team_ids};

/// `teamId` is `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="DeleteSlackWorkspaceConfiguration")`).
const TEAM_ID_MIN_LEN: usize = 1;
const TEAM_ID_MAX_LEN: usize = 256;

/// The template a miss on a Slack workspace configuration is reported with: the
/// resource and the member that identifies it, exactly as a channel miss is worded.
fn not_found(team_id: &str) -> String {
    format!("Slack workspace configuration for team {team_id} not found")
}

/// Require `reply` to be the modelled 404 for `team_id`, message included.
fn assert_not_found(case: &str, team_id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ResourceNotFoundException",
        "case {case:?}: expected x-amzn-errortype ResourceNotFoundException, got {:?}",
        reply.error_type
    );
    let message = reply.string("message").unwrap_or_default();
    let expected = not_found(team_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// Require `reply` to be the modelled 409 a channel create answers when the channel's
/// workspace is not registered to the account.
fn assert_workspace_conflict(case: &str, team_id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 409,
        "case {case:?}: expected HTTP 409, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ConflictException",
        "case {case:?}: expected x-amzn-errortype ConflictException, got {:?}",
        reply.error_type
    );
    let message = reply.string("message").unwrap_or_default();
    assert!(
        message.contains(team_id),
        "case {case:?}: expected a message naming team {team_id:?}, got {message:?}"
    );
}

/// The team ids the workspace listing reports on its first page, which is every
/// registered workspace while the account holds fewer than a page of them.
async fn listed_team_ids(emulator: &Emulator, case: &str) -> Vec<String> {
    let page = list_workspaces(emulator, None).await;
    assert_eq!(
        page.status, 200,
        "case {case:?}: the workspace listing must answer HTTP 200, got {} / {}",
        page.status, page.body
    );
    assert!(
        page.string("nextToken").is_none(),
        "case {case:?}: these tables register fewer than a page, so the listing must \
         offer no nextToken, got {}",
        page.body
    );
    reported_team_ids(&reported(&page))
}

/// A channel create body for `(team_id, channel_id)` that satisfies every modelled
/// constraint and the documented severity rule, so the only thing that can refuse it
/// is the workspace precondition.
fn channel_body(team_id: &str, channel_id: &str) -> Value {
    json!({
        "teamId": team_id,
        "channelId": channel_id,
        "notifyOnCaseSeverity": "none",
        "channelRoleArn": role_arn("SupportAppRole"),
    })
}

/// One row of the delete table: which workspace is deleted, and how much the account
/// holds in it.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    team_id: String,
    /// The channel configurations the workspace holds when it is deleted — none, one,
    /// or several.
    channel_ids: Vec<String>,
}

/// The delete table. Rows differ in `teamId` form and length — the recorded Slack
/// shape, both published `@length` bounds, punctuation and mixed case — and in how
/// many channel configurations the deleted workspace holds.
fn delete_cases() -> Vec<Case> {
    vec![
        Case {
            label: "the recorded teamId, one channel configured",
            team_id: "T012ABCDEFG".to_string(),
            channel_ids: vec!["C01234A5BCD".to_string()],
        },
        Case {
            label: "the shortest legal teamId, no channel configured",
            team_id: "T".repeat(TEAM_ID_MIN_LEN),
            channel_ids: Vec::new(),
        },
        Case {
            label: "a mixed-case punctuated teamId, three channels configured",
            team_id: "T-mixed.Case_9".to_string(),
            channel_ids: vec![
                "C-one".to_string(),
                "C-two".to_string(),
                "C-three".to_string(),
            ],
        },
        Case {
            label: "the longest legal teamId, one channel configured",
            team_id: "T".repeat(TEAM_ID_MAX_LEN),
            channel_ids: vec!["C".repeat(TEAM_ID_MAX_LEN)],
        },
    ]
}

#[tokio::test]
async fn deleting_a_registered_workspace_takes_it_and_its_channels_out_of_the_account() {
    // A second registered workspace in every row, so each assertion is about the
    // workspace the request named and not about an emptied account.
    let neighbour_team = "T999ZZZZZZZ";
    let neighbour_channel = "C999ZZZZZZZ";

    for case in delete_cases() {
        let label = case.label;
        let emulator = Emulator::new();

        for team in [case.team_id.as_str(), neighbour_team] {
            assert_eq!(
                emulator.register_workspace(team).await.status,
                200,
                "case {label:?}: the workspace must be registered"
            );
        }
        for channel_id in case.channel_ids.iter().map(String::as_str) {
            let created = emulator
                .create(channel_body(&case.team_id, channel_id))
                .await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: the fixture channel must be created, got {} / {}",
                created.status, created.body
            );
        }
        let created = emulator
            .create(channel_body(neighbour_team, neighbour_channel))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the neighbour's channel must be created, got {} / {}",
            created.status, created.body
        );

        // Both workspaces are in the registry the delete looks up.
        let mut before = vec![case.team_id.clone(), neighbour_team.to_string()];
        before.sort();
        assert_eq!(
            listed_team_ids(&emulator, label).await,
            before,
            "case {label:?}: expected both registrations to be listed before the delete"
        );

        let deleted = emulator.delete_workspace(&case.team_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected the delete to answer HTTP 200, got {} / {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {label:?}: the output shape has no members, got {}",
            deleted.body
        );

        // The account no longer holds that workspace: the listing reports only the
        // neighbour, deleting it again is the modelled miss, and a channel in it is
        // refused by the create precondition.
        assert_eq!(
            listed_team_ids(&emulator, label).await,
            vec![neighbour_team.to_string()],
            "case {label:?}: expected only the neighbour to be listed after the delete"
        );
        let twice = emulator.delete_workspace(&case.team_id).await;
        assert_not_found(&format!("{label} / second delete"), &case.team_id, &twice);
        let refused = emulator
            .create(channel_body(&case.team_id, "C-after-the-delete"))
            .await;
        assert_workspace_conflict(
            &format!("{label} / channel after the delete"),
            &case.team_id,
            &refused,
        );

        // A channel configuration lives inside a registered workspace, so the deleted
        // workspace's configurations went with it.
        for channel_id in case.channel_ids.iter().map(String::as_str) {
            assert!(
                emulator.listed(&case.team_id, channel_id).await.is_none(),
                "case {label:?}: expected channel {channel_id:?} to go with its \
                 workspace, got {}",
                emulator.list(None).await.body
            );
        }

        // And nothing else in the account moved: the other workspace is still
        // registered and still holds its channel.
        assert!(
            emulator
                .listed(neighbour_team, neighbour_channel)
                .await
                .is_some(),
            "case {label:?}: expected the neighbour's channel to survive, got {}",
            emulator.list(None).await.body
        );
        let neighbours_second_channel = emulator
            .create(channel_body(neighbour_team, "C-neighbour-two"))
            .await;
        assert_eq!(
            neighbours_second_channel.status, 200,
            "case {label:?}: expected the neighbour workspace to still accept a \
             channel, got {} / {}",
            neighbours_second_channel.status, neighbours_second_channel.body
        );

        // The delete removed the registration, not the account's ability to hold it:
        // the same workspace can be registered again, and then accepts channels — with
        // none of the configurations the delete took with it.
        assert_eq!(
            emulator.register_workspace(&case.team_id).await.status,
            200,
            "case {label:?}: the workspace must be registrable again"
        );
        assert_eq!(
            listed_team_ids(&emulator, label).await,
            before,
            "case {label:?}: expected the re-registered workspace to be listed again"
        );
        for channel_id in case.channel_ids.iter().map(String::as_str) {
            assert!(
                emulator.listed(&case.team_id, channel_id).await.is_none(),
                "case {label:?}: re-registering the workspace must not resurrect \
                 channel {channel_id:?}, got {}",
                emulator.list(None).await.body
            );
            let recreated = emulator
                .create(channel_body(&case.team_id, channel_id))
                .await;
            assert_eq!(
                recreated.status, 200,
                "case {label:?}: expected channel {channel_id:?} to be creatable again, \
                 got {} / {}",
                recreated.status, recreated.body
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_workspace_the_account_never_registered_is_not_found() {
    let emulator = Emulator::new();
    let registered = "T012ABCDEFG";
    assert_eq!(
        emulator.register_workspace(registered).await.status,
        200,
        "the neighbour workspace must be registered"
    );

    let longest = "T".repeat(TEAM_ID_MAX_LEN);
    // Rows differ in the form and length of a syntactically valid `teamId` the account
    // holds no configuration for: a Slack-shaped id, both published length bounds,
    // punctuation, and ids that are a prefix and an extension of the registered one —
    // a lookup is by the whole id, not by a prefix of it.
    let rows: [(&str, &str); 6] = [
        ("a Slack-shaped id", "T999ZZZZZZZ"),
        ("the shortest legal id", "T"),
        ("the longest legal id", longest.as_str()),
        ("a punctuated id", "T-0_1.2"),
        ("a prefix of the registered id", "T012ABCDEF"),
        ("an extension of the registered id", "T012ABCDEFGH"),
    ];
    for (label, team_id) in rows {
        // Twice: a refused delete changes nothing, so it answers the same thing again.
        for attempt in 1..=2 {
            let refused = emulator.delete_workspace(team_id).await;
            assert_not_found(&format!("{label} / attempt {attempt}"), team_id, &refused);
        }
        // Nor did it register the workspace it could not find, or disturb the registry.
        let channel = emulator.create(channel_body(team_id, "C01234A5BCD")).await;
        assert_workspace_conflict(&format!("{label} / channel"), team_id, &channel);
        assert_eq!(
            listed_team_ids(&emulator, label).await,
            vec![registered.to_string()],
            "case {label:?}: expected the registry to still hold only the registered \
             workspace"
        );
    }

    // The registered workspace is untouched by all of that, and deleting IT succeeds.
    let channel = emulator
        .create(channel_body(registered, "C01234A5BCD"))
        .await;
    assert_eq!(
        channel.status, 200,
        "expected the registered workspace to still accept a channel, got {} / {}",
        channel.status, channel.body
    );
    assert_eq!(
        emulator.delete_workspace(registered).await.status,
        200,
        "expected the registered workspace to be deletable"
    );
    assert_eq!(
        listed_team_ids(&emulator, "the emptied account").await,
        Vec::<String>::new(),
        "expected the account to hold no workspace after the last delete"
    );
}

#[tokio::test]
async fn deleting_a_workspace_frees_a_slot_in_the_documented_quota() {
    let emulator = Emulator::new();
    // Fill the documented registry quota. Rows differ in id form and length, as above.
    let teams: Vec<String> = vec![
        "T012ABCDEFG".to_string(),
        "T".to_string(),
        "T-mixed.Case_9".to_string(),
        "T".repeat(TEAM_ID_MAX_LEN),
        "T0000000000".to_string(),
    ];
    assert_eq!(
        teams.len(),
        MAX_SLACK_WORKSPACE_CONFIGURATIONS,
        "the table must fill exactly the documented workspace quota"
    );
    for team in &teams {
        assert_eq!(
            emulator.register_workspace(team).await.status,
            200,
            "case {:?}: the workspace must be registered",
            &team[..team.len().min(16)]
        );
    }

    // One past the quota is refused while the registry is full.
    let newcomer = "T777NEWONEXX";
    let refused = emulator.register_workspace(newcomer).await;
    assert_eq!(
        refused.status, 400,
        "expected HTTP 400 past the workspace quota, got {} / {}",
        refused.status, refused.body
    );

    // Deleting any one of the registered workspaces makes room for exactly one more:
    // the quota counts what the registry HOLDS.
    for team in &teams {
        let label = &team[..team.len().min(16)];
        assert_eq!(
            emulator.delete_workspace(team).await.status,
            200,
            "case {label:?}: the workspace must be deletable"
        );
        let accepted = emulator.register_workspace(newcomer).await;
        assert_eq!(
            accepted.status, 200,
            "case {label:?}: expected the freed slot to accept a registration, got {} / {}",
            accepted.status, accepted.body
        );
        // The registry is full again, so the next newcomer is refused — and the
        // deleted workspace is one of those newcomers.
        let full_again = emulator.register_workspace(team).await;
        assert_eq!(
            full_again.status, 400,
            "case {label:?}: expected the registry to be full again, got {} / {}",
            full_again.status, full_again.body
        );
        // Put the registry back the way the table found it for the next row.
        assert_eq!(
            emulator.delete_workspace(newcomer).await.status,
            200,
            "case {label:?}: the newcomer must be deletable"
        );
        assert_eq!(
            emulator.register_workspace(team).await.status,
            200,
            "case {label:?}: the row's workspace must be registrable again"
        );
    }
}
