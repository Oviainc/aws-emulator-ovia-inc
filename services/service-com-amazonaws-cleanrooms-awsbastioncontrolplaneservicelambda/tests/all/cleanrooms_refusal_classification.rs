// SPDX-License-Identifier: CC0-1.0

//! How Clean Rooms CLASSIFIES a refusal: which modelled error shape, which HTTP status,
//! and what the message is composed from.
//!
//! Three rules are under test, each asserted over a TABLE whose rows differ along what
//! the rule generalises over rather than re-running one recorded request:
//!
//! 1. `CreateMembership` by an account that already holds a membership in the
//!    collaboration is an input-validation refusal — `ValidationException` (400) —
//!    naming the account the membership would have been created for. It is terminal,
//!    not the retryable resource conflict a 409 announces.
//! 2. A `collaborationIdentifier` the caller is not authorized to is an authorization
//!    refusal — `AccessDeniedException` (403) — on every operation that takes one, and
//!    the three indistinguishable cases (never created, deleted, caller not part of it)
//!    get the same answer: authorization to a collaboration lives in the collaboration
//!    and disappears with it, and `GetCollaboration` models no not-found error at all.
//! 3. A membership lookup that finds nothing reports a message composed from the
//!    requested `membershipIdentifier` AND the resolved caller account, because
//!    membership ids are account-scoped.
//!
//! Every request goes over the wire through the same `MultiServiceRouter` the production
//! router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, error_message, minimal_collaboration, Emulator, ALL_LOG_STATUSES,
    ALL_MEMBER_ABILITIES,
};

/// An account id that is not the caller's, used to make a stored collaboration one the
/// caller is not part of. Twelve digits, the form `AccountId` declares.
const FOREIGN_ACCOUNT: &str = "210987654321";

/// Syntactically valid collaboration/membership ids (the model's `UUID` form) that no
/// create in these tests mints.
const NEVER_MINTED_IDS: [&str; 3] = [
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "0123456f-89ab-cdef-0123-456789abcdef",
];

/// One row of the duplicate-join table: a collaboration to create, the membership
/// request to repeat, and how many times to repeat it.
struct JoinCase {
    label: &'static str,
    collaboration: Value,
    membership: Value,
    repeats: usize,
}

fn join_cases() -> Vec<JoinCase> {
    vec![
        JoinCase {
            label: "minimal collaboration, minimal membership, one repeat",
            collaboration: minimal_collaboration("dup-minimal"),
            membership: json!({ "queryLogStatus": "ENABLED" }),
            repeats: 1,
        },
        JoinCase {
            label: "every member ability, membership with logs/metrics/tags, three repeats",
            collaboration: json!({
                "name": "dup-rich",
                "description": "duplicate join",
                "creatorDisplayName": "rich-creator",
                "creatorMemberAbilities": ALL_MEMBER_ABILITIES,
                "members": [],
                "queryLogStatus": ALL_LOG_STATUSES[1],
            }),
            membership: json!({
                "queryLogStatus": ALL_LOG_STATUSES[0],
                "jobLogStatus": ALL_LOG_STATUSES[1],
                "isMetricsEnabled": true,
                "tags": { "team": "data" },
                "paymentConfiguration": {
                    "queryCompute": { "isResponsible": true },
                },
            }),
            repeats: 3,
        },
        JoinCase {
            label: "collaboration with another declared member, two repeats",
            collaboration: json!({
                "name": "dup-with-members",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_QUERY", "CAN_RECEIVE_RESULTS"],
                "members": [{
                    "accountId": FOREIGN_ACCOUNT,
                    "displayName": "invited",
                    "memberAbilities": [],
                }],
                "queryLogStatus": ALL_LOG_STATUSES[0],
            }),
            membership: json!({
                "queryLogStatus": ALL_LOG_STATUSES[1],
                "isMetricsEnabled": false,
            }),
            repeats: 2,
        },
    ]
}

#[tokio::test]
async fn a_repeat_join_is_refused_as_input_validation_naming_the_account() {
    let emulator = Emulator::new();

    for case in join_cases() {
        let (status, created) = emulator.create_collaboration(case.collaboration.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateCollaboration failed with {status}: {created}",
            case.label
        );
        let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();

        let mut body = case.membership.clone();
        body["collaborationIdentifier"] = json!(collaboration_id);
        let (status, joined) = emulator.create_membership(body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first join must succeed, got {status} with {joined}",
            case.label
        );
        let membership = joined["membership"].clone();
        let membership_id = membership["id"].as_str().unwrap().to_string();

        for attempt in 1..=case.repeats {
            let (status, refusal) = emulator.create_membership(body.clone()).await;
            assert_eq!(
                status, 400,
                "case {:?} attempt {attempt}: expected 400, got {status} with {refusal}",
                case.label
            );
            assert_eq!(
                error_code(&refusal),
                "ValidationException",
                "case {:?} attempt {attempt}: expected ValidationException, got {refusal}",
                case.label
            );
            // Composed from the caller account the membership would be created for and
            // the collaboration identifier the request carried.
            let message = error_message(&refusal);
            assert!(
                message.contains(account_id()) && message.contains(&collaboration_id),
                "case {:?} attempt {attempt}: the message must name the account and the \
                 collaboration, got {message:?}",
                case.label
            );
            // And the refusal is terminal: nothing was created, nothing was replaced.
            let (status, fetched) = emulator.get_membership(&membership_id).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: the first membership must survive, got {status} \
                 with {fetched}",
                case.label
            );
            assert_eq!(
                fetched["membership"], membership,
                "case {:?} attempt {attempt}: a refused join must not change the membership, \
                 got {:?}",
                case.label, fetched["membership"]
            );
        }

        // One membership exists for the pair, whatever the number of attempts.
        let (_, list) = emulator.call("GET", "/memberships", None).await;
        let held: Vec<&str> = list["membershipSummaries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|summary| summary["collaborationId"] == json!(collaboration_id))
            .map(|summary| summary["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            held,
            vec![membership_id.as_str()],
            "case {:?}: exactly one membership must exist for the pair, got {held:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_collaboration_the_caller_cannot_reach_is_refused_as_access_denied() {
    let emulator = Emulator::new();

    // A collaboration the caller IS authorized to, as the control for every row: the
    // gate must refuse only what it should.
    let (_, reachable) = emulator
        .create_collaboration(minimal_collaboration("reachable"))
        .await;
    let reachable_id = reachable["collaboration"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Rows span the three ways a collaboration identifier is unreachable, plus the
    // repeat of a delete (DeleteCollaboration is @idempotent, so the second one also
    // succeeds and the read after it answers the same).
    let mut unreachable: Vec<(String, String)> = NEVER_MINTED_IDS
        .iter()
        .map(|id| (format!("never minted {id}"), id.to_string()))
        .collect();

    for (label, deletes) in [("deleted once", 1usize), ("deleted twice", 2)] {
        let (_, created) = emulator
            .create_collaboration(minimal_collaboration(label))
            .await;
        let id = created["collaboration"]["id"].as_str().unwrap().to_string();
        // Joined first, so the row also covers the cascade: the membership survives the
        // collaboration while the collaboration itself becomes unreachable.
        let (status, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the join must succeed, got {status} with {joined}"
        );
        let membership_id = joined["membership"]["id"].as_str().unwrap().to_string();
        for pass in 1..=deletes {
            let (status, body) = emulator.delete_collaboration(&id).await;
            assert_eq!(
                status, 204,
                "case {label:?} delete {pass}: expected 204, got {status} with {body}"
            );
        }
        let (status, body) = emulator.get_membership(&membership_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the membership must survive its collaboration, got {status} \
             with {body}"
        );
        assert_eq!(
            body["membership"]["status"].as_str(),
            Some("COLLABORATION_DELETED"),
            "case {label:?}: expected status COLLABORATION_DELETED, got {:?}",
            body["membership"]["status"]
        );
        unreachable.push((label.to_string(), id));
    }

    // A stored collaboration the caller is not part of: created over the wire, then
    // re-owned in the store so neither its creator nor its members name this account.
    {
        let (_, created) = emulator
            .create_collaboration(minimal_collaboration("owned-elsewhere"))
            .await;
        let id = created["collaboration"]["id"].as_str().unwrap().to_string();
        let store = emulator.store();
        let mut store = store.lock();
        let record = store
            .collaborations
            .get_mut(&id)
            .expect("the collaboration just created");
        record.creator_account_id = FOREIGN_ACCOUNT.to_string();
        record.members.clear();
        drop(store);
        unreachable.push(("stored but not this account's".to_string(), id));
    }

    for (label, collaboration_id) in unreachable {
        // GetCollaboration: the model declares no not-found error, so the refusal is
        // the authorization one.
        let (status, body) = emulator.get_collaboration(&collaboration_id).await;
        assert_eq!(
            status, 403,
            "case GetCollaboration {label:?}: expected 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case GetCollaboration {label:?}: expected AccessDeniedException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(account_id()) && message.contains(&collaboration_id),
            "case GetCollaboration {label:?}: the message must name the account and the \
             collaboration, got {message:?}"
        );

        // CreateMembership resolves the same identifier through the same gate.
        let (status, body) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "DISABLED",
            }))
            .await;
        assert_eq!(
            status, 403,
            "case CreateMembership {label:?}: expected 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case CreateMembership {label:?}: expected AccessDeniedException, got {body}"
        );

        // The reachable collaboration is unaffected by every refusal above.
        let (status, body) = emulator.get_collaboration(&reachable_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the authorized collaboration must still answer, got {status} \
             with {body}"
        );
        assert_eq!(
            body["collaboration"]["id"].as_str(),
            Some(reachable_id.as_str()),
            "case {label:?}: expected {reachable_id}, got {:?}",
            body["collaboration"]["id"]
        );
    }
}

#[tokio::test]
async fn a_membership_lookup_refusal_names_the_id_and_the_caller_account() {
    let emulator = Emulator::new();

    // Rows: ids no create minted, and an id that WAS minted and then deleted — the
    // second kind proves the message is composed per request rather than stored.
    let mut absent: Vec<(String, String)> = NEVER_MINTED_IDS
        .iter()
        .map(|id| (format!("never minted {id}"), id.to_string()))
        .collect();
    {
        let (_, created) = emulator
            .create_collaboration(minimal_collaboration("deleted-membership"))
            .await;
        let collaboration_id = created["collaboration"]["id"].as_str().unwrap().to_string();
        let (_, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        let membership_id = joined["membership"]["id"].as_str().unwrap().to_string();
        let (status, body) = emulator.delete_membership(&membership_id).await;
        assert_eq!(
            status, 204,
            "case deleted membership: expected DeleteMembership 204, got {status} with {body}"
        );
        absent.push(("minted then deleted".to_string(), membership_id));
    }

    for (label, membership_id) in absent {
        // The composition rule: the requested identifier AND the account whose
        // namespace was searched.
        let expected = format!(
            "No such membership: {membership_id} for account {}",
            account_id()
        );
        for (operation, (status, body)) in [
            ("GetMembership", emulator.get_membership(&membership_id).await),
            (
                "DeleteMembership",
                emulator.delete_membership(&membership_id).await,
            ),
        ] {
            assert_eq!(
                status, 404,
                "case {operation} {label:?}: expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation} {label:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {operation} {label:?}: the message must name the requested id and the \
                 caller account, got {:?}",
                error_message(&body)
            );
        }
    }
}
