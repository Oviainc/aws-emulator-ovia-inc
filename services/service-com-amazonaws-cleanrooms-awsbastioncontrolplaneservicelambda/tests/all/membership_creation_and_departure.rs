// SPDX-License-Identifier: CC0-1.0

//! When a membership may be CREATED and when it may be DELETED.
//!
//! The rules under test:
//!
//! * a membership is creatable only for a member the collaboration still has
//!   outstanding — one in `INVITED` state — so an account that already holds one (and is
//!   therefore `ACTIVE`) is refused with `ValidationException` (400) whose message names
//!   that account and the state a member has to be in;
//! * a membership is deletable only once nothing depends on it: the collaboration an
//!   account CREATED depends on that account's membership, so the creator cannot leave
//!   its own live collaboration (`ConflictException`, 409) while every other party may
//!   leave at any time, and the creator's membership becomes deletable once the
//!   collaboration is gone.
//!
//! The rows vary what the rules generalise over: how many other members the
//! collaboration declares, which abilities the caller holds, how many repeat joins are
//! attempted, and the caller's relation to the collaboration (its creator or one of its
//! declared members). Every request goes over the wire through the same
//! `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, error_message, minimal_collaboration,
    Emulator, ALL_LOG_STATUSES, ALL_MEMBER_ABILITIES,
};

/// The message the INVITED-state rule prescribes for `account`.
fn not_invited(account: &str) -> String {
    format!(
        "Cannot create membership for account: {account}. Memberships can only be created \
         for members in INVITED state"
    )
}

/// Create a collaboration for one row and return its id.
async fn collaboration(emulator: &Emulator, name: &str, abilities: &[&str], members: Value) -> String {
    let mut body = minimal_collaboration(name);
    body["creatorMemberAbilities"] = json!(abilities);
    body["members"] = members;
    let (status, created) = emulator.create_collaboration(body).await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no collaboration.id in {created}"))
        .to_string()
}

/// One row of the create table: the collaboration to create and how many repeat joins to
/// attempt after the first one succeeds.
struct JoinCase {
    label: &'static str,
    abilities: Vec<&'static str>,
    members: Value,
    repeats: usize,
}

#[tokio::test]
async fn a_membership_is_creatable_only_for_a_member_in_invited_state() {
    let cases = vec![
        JoinCase {
            label: "one ability, no other members, one repeat",
            abilities: vec![ALL_MEMBER_ABILITIES[0]],
            members: json!([]),
            repeats: 1,
        },
        JoinCase {
            label: "every ability, two other members, three repeats",
            abilities: ALL_MEMBER_ABILITIES.to_vec(),
            members: json!([
                {
                    "accountId": "111111111111",
                    "memberAbilities": [ALL_MEMBER_ABILITIES[1]],
                    "displayName": "partner-a",
                },
                {
                    "accountId": "222222222222",
                    "memberAbilities": [],
                    "displayName": "partner-b",
                },
            ]),
            repeats: 3,
        },
    ];

    for case in cases {
        let label = case.label;
        let emulator = Emulator::new();
        let collaboration_id =
            collaboration(&emulator, label, &case.abilities, case.members.clone()).await;
        let join = json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
        });

        let (status, joined) = emulator.create_membership(join.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first join must succeed, got {status} with {joined}"
        );
        let membership = joined["membership"].clone();

        for attempt in 1..=case.repeats {
            let (status, refused) = emulator.create_membership(join.clone()).await;
            assert_eq!(
                status, 400,
                "case {label:?} attempt {attempt}: expected 400, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {label:?} attempt {attempt}: expected ValidationException, got {refused}"
            );
            assert_eq!(
                error_message(&refused),
                not_invited(account_id()),
                "case {label:?} attempt {attempt}: expected the INVITED-state wording, got {:?}",
                error_message(&refused)
            );
            // The refusal is terminal: the first membership is untouched.
            let (status, fetched) = emulator
                .get_membership(membership["id"].as_str().unwrap())
                .await;
            assert_eq!(
                status, 200,
                "case {label:?} attempt {attempt}: the first membership must survive, got \
                 {status} with {fetched}"
            );
            assert_eq!(
                fetched["membership"], membership,
                "case {label:?} attempt {attempt}: a refused join must change nothing, got {:?}",
                fetched["membership"]
            );
        }
    }
}

#[tokio::test]
async fn the_creator_cannot_leave_its_own_live_collaboration_but_a_participant_can() {
    // Rows differ in the caller's relation to the collaboration: its creator (the
    // collaboration depends on that membership) and one of its declared members (which
    // may leave). The abilities differ too, so the rule is about the relation and not
    // about what the member may do.
    for (row, abilities) in [
        vec![ALL_MEMBER_ABILITIES[0]],
        ALL_MEMBER_ABILITIES.to_vec(),
    ]
    .into_iter()
    .enumerate()
    {
        let label = format!("abilities {abilities:?}");
        let emulator = Emulator::new();

        // --- the creator ---
        let own_id = collaboration(
            &emulator,
            &format!("creator-leave-{row}"),
            &abilities,
            json!([]),
        )
        .await;
        let (status, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": own_id,
                "queryLogStatus": ALL_LOG_STATUSES[0],
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the creator's join must succeed, got {status} with {joined}"
        );
        let own_membership_id = joined["membership"]["id"].as_str().unwrap().to_string();

        let (status, refused) = emulator.delete_membership(&own_membership_id).await;
        assert_eq!(
            status, 409,
            "case {label:?}: the creator must not be able to leave, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            refused["reason"].as_str(),
            Some("INVALID_STATE"),
            "case {label:?}: expected reason INVALID_STATE, got {:?}",
            refused["reason"]
        );
        assert_eq!(
            error_message(&refused),
            "Cannot delete membership for the creator of the collaboration while their \
             membership is still ACTIVE.",
            "case {label:?}: the refusal states the rule in fixed text, interpolating no \
             membership or collaboration id, got {:?}",
            error_message(&refused)
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(own_membership_id.as_str()),
            "case {label:?}: the refusal must name the membership it is about in \
             resourceId, got {:?}",
            refused["resourceId"]
        );
        // The refusal changed nothing: the membership is still there and still ACTIVE.
        let (status, fetched) = emulator.get_membership(&own_membership_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the refused delete must leave the membership, got {status} \
             with {fetched}"
        );
        assert_eq!(
            fetched["membership"]["status"].as_str(),
            Some("ACTIVE"),
            "case {label:?}: expected status ACTIVE, got {:?}",
            fetched["membership"]["status"]
        );

        // Once the collaboration is gone, nothing depends on the membership any more and
        // the delete is accepted.
        let (status, body) = emulator.delete_collaboration(&own_id).await;
        assert_eq!(
            status, 204,
            "case {label:?}: the owner's DeleteCollaboration must succeed, got {status} \
             with {body}"
        );
        let (status, body) = emulator.delete_membership(&own_membership_id).await;
        assert_eq!(
            status, 204,
            "case {label:?}: a membership whose collaboration is gone must be deletable, got \
             {status} with {body}"
        );

        // --- a declared member of someone else's collaboration ---
        let other_id = collaboration_owned_elsewhere(
            &emulator,
            &format!("participant-leave-{row}"),
            Some(&abilities),
        )
        .await;
        let (status, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": other_id,
                "queryLogStatus": ALL_LOG_STATUSES[1],
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a declared member's join must succeed, got {status} with {joined}"
        );
        let participant_membership_id = joined["membership"]["id"].as_str().unwrap().to_string();

        let (status, body) = emulator.delete_membership(&participant_membership_id).await;
        assert_eq!(
            status, 204,
            "case {label:?}: a participant must be able to leave, got {status} with {body}"
        );
        let (status, body) = emulator.get_membership(&participant_membership_id).await;
        assert_eq!(
            status, 404,
            "case {label:?}: the membership it left must be gone, got {status} with {body}"
        );

        // And having left, it is INVITED again, so it may join afresh.
        let (status, rejoined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": other_id,
                "queryLogStatus": ALL_LOG_STATUSES[1],
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a member that left is INVITED again, got {status} with {rejoined}"
        );
        assert_ne!(
            rejoined["membership"]["id"].as_str(),
            Some(participant_membership_id.as_str()),
            "case {label:?}: a rejoin must mint a fresh id, got {participant_membership_id} again"
        );
    }
}
