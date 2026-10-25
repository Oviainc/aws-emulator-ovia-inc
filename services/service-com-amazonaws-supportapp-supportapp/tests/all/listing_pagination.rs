// SPDX-License-Identifier: CC0-1.0

//! The pagination rule both Support App listings follow.
//!
//! Neither `ListSlackChannelConfigurations` nor `ListSlackWorkspaceConfigurations`
//! takes a page-size member, and each reports a collection the service's own documented
//! account quota bounds (20 Slack channel configurations, 5 Slack workspaces), so a
//! listing answers HTTP 200 with EVERY stored record and omits `nextToken` — the member
//! documents that the response carries no token once it returns the last set of
//! results. Nothing is ever withheld, so a request that carries a `nextToken` names no
//! position and is refused 400 `ValidationException`.
//!
//! The tables below span what that rule generalises over: WHICH listing (both of them,
//! each against its own quota), how many records are stored (none, one, several, both
//! sides of the quota's upper end, and the quota itself — including the counts a page
//! boundary would have fallen on), the history that left the store at that count
//! (registrations, deletions and re-registrations, so each count is reached from below
//! and from above), and the length of the stored ids. The recorded twelve-configuration
//! case is one row.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`], so a reply
//! that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    MAX_SLACK_CHANNEL_CONFIGURATIONS, MAX_SLACK_WORKSPACE_CONFIGURATIONS,
};

use crate::slack_channel_configurations::{role_arn, Emulator};
use crate::slack_workspace_configurations::{list_workspaces, reported, reported_team_ids};

/// The `teamId` of the workspace whose channels the channel table configures.
const CHANNEL_TEAM_ID: &str = "T012ABCDEFG";

/// Register `count` Slack workspaces and return their team ids in `teamId` order.
///
/// The ids differ in length as well as in value, so no row's answer is the same size as
/// another's.
async fn register_workspaces(emulator: &Emulator, count: usize, label: &str) -> Vec<String> {
    let mut team_ids = Vec::new();
    for index in 0..count {
        let team_id = format!("T{index:03}{}", "W".repeat(index * 7));
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: workspace {index} must be registered"
        );
        team_ids.push(team_id);
    }
    team_ids.sort();
    team_ids
}

/// Store `count` Slack channel configurations in one registered workspace and return
/// their channel ids in `channelId` order.
async fn store_configurations(emulator: &Emulator, count: usize, label: &str) -> Vec<String> {
    let mut channel_ids = Vec::new();
    for index in 0..count {
        // `channelId` is `len 1..256`, so the rows grow towards that bound rather than
        // all being the same size.
        let channel_id = format!("C{index:03}{}", "c".repeat(index * 9));
        let created = emulator
            .create_in_registered_workspace(json!({
                "teamId": CHANNEL_TEAM_ID,
                "channelId": channel_id,
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: configuration {index} must be created, got {} / {}",
            created.status, created.body
        );
        channel_ids.push(channel_id);
    }
    channel_ids.sort();
    channel_ids
}

#[tokio::test]
async fn the_channel_listing_reports_every_stored_configuration_without_a_token() {
    // Rows: an empty account, one configuration, the counts a ten-record page boundary
    // would have fallen either side of, the twelve of the recorded case, one short of
    // the documented quota, and the quota itself.
    for stored in [0usize, 1, 9, 10, 11, 12, 19, MAX_SLACK_CHANNEL_CONFIGURATIONS] {
        let label = format!("{stored} configurations stored");
        let emulator = Emulator::new();
        let expected = store_configurations(&emulator, stored, &label).await;

        let reply = emulator.list(None).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        assert!(
            reply.string("nextToken").is_none(),
            "case {label:?}: expected no nextToken, got {}",
            reply.body
        );
        let channel_ids: Vec<String> = reply
            .configurations()
            .iter()
            .map(|item| {
                item.get("channelId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            channel_ids, expected,
            "case {label:?}: expected every stored configuration in the one response, got \
             {} entries",
            channel_ids.len()
        );
    }
}

#[tokio::test]
async fn the_workspace_listing_reports_every_registered_workspace_without_a_token() {
    // Rows: an empty registry, one workspace, the counts a two-record page boundary
    // would have fallen either side of, one short of the documented quota, and the quota
    // itself.
    for registered in [0usize, 1, 2, 3, 4, MAX_SLACK_WORKSPACE_CONFIGURATIONS] {
        let label = format!("{registered} workspaces registered");
        let emulator = Emulator::new();
        let expected = register_workspaces(&emulator, registered, &label).await;

        let reply = list_workspaces(&emulator, None).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        assert!(
            reply.string("nextToken").is_none(),
            "case {label:?}: expected no nextToken, got {}",
            reply.body
        );
        let records = reported(&reply);
        assert_eq!(
            reported_team_ids(&records),
            expected,
            "case {label:?}: expected every registered workspace in the one response, got \
             {:?}",
            reported_team_ids(&records)
        );
    }
}

/// One step of a registry history: what the account is asked to do next, naming a
/// workspace by its index in [`crate::slack_workspace_configurations::team_ids`].
#[derive(Debug, Clone, Copy)]
enum Step {
    Register(usize),
    Delete(usize),
}

#[tokio::test]
async fn the_workspace_listing_reports_the_whole_registry_after_every_change() {
    use Step::{Delete, Register};

    // The rule is an invariant: whatever the registry holds at this instant is what the
    // listing reports, whole and with no token. The rows below are histories that take
    // the registry up to the documented quota and back down again, so the invariant is
    // asserted at every count in BOTH directions — including the counts a page bounded
    // below the quota would have split — and after a deletion, a re-registration and a
    // registration of a workspace the account already holds, which a table that only
    // ever registers distinct workspaces never reaches.
    let ids = crate::slack_workspace_configurations::team_ids();
    let fill: Vec<Step> = (0..ids.len()).map(Register).collect();
    let drain: Vec<Step> = (0..ids.len()).map(Delete).collect();
    let histories: [(&str, Vec<Step>); 5] = [
        ("fills the registry", fill.clone()),
        (
            "fills the registry, then empties it",
            fill.into_iter().chain(drain).collect(),
        ),
        (
            "grows past a deletion",
            vec![
                Register(0),
                Register(1),
                Register(2),
                Delete(1),
                Register(3),
                Register(4),
                Delete(0),
            ],
        ),
        (
            "re-registers what it deleted",
            vec![Register(2), Register(4), Delete(2), Register(2), Register(0)],
        ),
        (
            // Registering a workspace the account already holds changes nothing, so the
            // repeated registration leaves the registry — and therefore the listing —
            // exactly as it was.
            "registers a workspace it already holds",
            vec![
                Register(1),
                Register(1),
                Register(3),
                Register(1),
                Register(3),
                Delete(1),
                Register(3),
            ],
        ),
    ];

    for (label, history) in histories {
        let emulator = Emulator::new();
        // The registry this history has built so far, which is what every listing below
        // must report.
        let mut registry: Vec<String> = Vec::new();

        for (position, step) in history.iter().enumerate() {
            let case = format!("{label:?} step {position} {step:?}");
            match *step {
                Register(index) => {
                    let registered = emulator.register_workspace(&ids[index]).await;
                    assert_eq!(
                        registered.status, 200,
                        "case {case}: expected the registration to answer HTTP 200, got {} / {}",
                        registered.status, registered.body
                    );
                    if !registry.contains(&ids[index]) {
                        registry.push(ids[index].clone());
                    }
                }
                Delete(index) => {
                    let deleted = emulator.delete_workspace(&ids[index]).await;
                    assert_eq!(
                        deleted.status, 200,
                        "case {case}: expected the deletion to answer HTTP 200, got {} / {}",
                        deleted.status, deleted.body
                    );
                    registry.retain(|team_id| team_id != &ids[index]);
                }
            }
            // The store is keyed by `teamId`, so the listing walks it in that order
            // whatever order the registrations arrived in.
            registry.sort();

            let reply = list_workspaces(&emulator, None).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            assert!(
                reply.string("nextToken").is_none(),
                "case {case}: expected no nextToken on a registry the workspace quota \
                 bounds, got {}",
                reply.body
            );
            let reported_ids = reported_team_ids(&reported(&reply));
            assert_eq!(
                reported_ids, registry,
                "case {case}: expected the whole registry of {} workspaces in the one \
                 response, got {reported_ids:?}",
                registry.len()
            );
        }
    }
}

#[tokio::test]
async fn neither_listing_returns_a_token_with_both_stores_at_their_quota() {
    // The largest answer either listing can give: each store filled to the quota that
    // bounds it, so there is no account state in which a listing is split.
    let emulator = Emulator::new();
    // One workspace short of the quota here: the channel table registers its own
    // workspace, which takes the registry to exactly the quota.
    register_workspaces(
        &emulator,
        MAX_SLACK_WORKSPACE_CONFIGURATIONS - 1,
        "both stores at quota",
    )
    .await;
    let stored = store_configurations(
        &emulator,
        MAX_SLACK_CHANNEL_CONFIGURATIONS,
        "both stores at quota",
    )
    .await;

    let workspaces = list_workspaces(&emulator, None).await;
    assert!(
        workspaces.string("nextToken").is_none(),
        "expected no nextToken from a registry at its quota, got {}",
        workspaces.body
    );
    assert_eq!(
        reported(&workspaces).len(),
        MAX_SLACK_WORKSPACE_CONFIGURATIONS,
        "expected every registered workspace in the one response, got {}",
        workspaces.body
    );

    let channels = emulator.list(None).await;
    assert!(
        channels.string("nextToken").is_none(),
        "expected no nextToken from an account at its channel quota, got {}",
        channels.body
    );
    assert_eq!(
        channels.configurations().len(),
        stored.len(),
        "expected every stored configuration in the one response, got {}",
        channels.body
    );
}

#[tokio::test]
async fn a_next_token_is_refused_by_both_listings() {
    let emulator = Emulator::new();
    // Stored records on both resources, so each refusal is about the token rather than
    // about an empty account.
    register_workspaces(&emulator, 3, "token rows").await;
    store_configurations(&emulator, 3, "token rows").await;

    // Rows span the forms a token could arrive in — plain text, hex of an id, hex of a
    // position, hex of bytes that are not text, and an id sent unencoded — because no
    // listing of this service returns one for a caller to send back.
    let tokens = [
        ("plain text", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of bytes that are not text", "fffe".to_string()),
        ("hex of a team id", hex::encode(CHANNEL_TEAM_ID)),
        ("hex of a position", hex::encode("2")),
        ("hex of a zero position", hex::encode("0")),
        ("an id sent unencoded", CHANNEL_TEAM_ID.to_string()),
    ];
    for (form, token) in tokens {
        for operation in [
            "ListSlackChannelConfigurations",
            "ListSlackWorkspaceConfigurations",
        ] {
            let label = format!("{operation} / {form}");
            let refused = if operation == "ListSlackChannelConfigurations" {
                emulator.list(Some(&token)).await
            } else {
                list_workspaces(&emulator, Some(&token)).await
            };
            assert_eq!(
                refused.status, 400,
                "case {label:?}: expected HTTP 400 for token {token:?}, got {} / {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type, "ValidationException",
                "case {label:?}: expected ValidationException, got {:?}",
                refused.error_type
            );
        }
    }

    // The same listings answer the same requests without a token, so the refusals are
    // about the member and not about the listing being refused outright.
    assert_eq!(
        emulator.list(None).await.configurations().len(),
        3,
        "expected the untokened channel listing to report every stored configuration"
    );
    assert_eq!(
        reported(&list_workspaces(&emulator, None).await).len(),
        // The channel table registered its own workspace too.
        4,
        "expected the untokened workspace listing to report every registered workspace"
    );
}
