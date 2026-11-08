// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFirewallRuleGroup`, the operation that retires a
//! DNS Firewall rule group, and for the readers its removal is observed through:
//! `GetFirewallRuleGroup` and `ListTagsForResource`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly; the store is reached directly only to put a rule
//! group into a status whose settling is still in flight.
//!
//! What these cases pin is the rule the operation implements: a delete reports
//! the stored record one last time, in the lifecycle step it last reached, and
//! drops it from the store, so every later read of that id reports it missing
//! and the identifiers it held are free again; a delete of a rule group whose
//! last change has not settled is refused and changes nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::FIREWALL_RULE_GROUP_STATUS_VALUES;
use service_com_amazonaws_route53resolver_route53resolver::state::{
    service_state, FIREWALL_RULE_GROUP_STATUS_COMPLETE, FIREWALL_RULE_GROUP_STATUS_DELETING,
    PROVISIONING_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator, emulator_on, error_type, expect_error, ok, parse_time, string_member, ACCOUNT,
    REGION,
};

/// What a refusal of a state transition states, from the model's documentation
/// of the error class
/// (`smithy_query(query="shape",shape="ConflictException")`): "The requested
/// state transition isn't valid. For example, you can't delete a firewall
/// domain list if it is in the process of being deleted". The sentence names no
/// resource, so every refusal of this class carries it unchanged.
const STATE_TRANSITION_CONFLICT: &str = "The requested state transition isn't valid.";

/// The sentence DNS Firewall states in `FirewallRuleGroup$StatusMessage` for a
/// group that reached its created, settled lifecycle step, and the one it
/// states once a delete has been accepted for the group: fixed text for the
/// step, with no part of the request in it.
const CREATED_RULE_GROUP_MESSAGE: &str = "Created Firewall Rule Group";
const DELETING_RULE_GROUP_MESSAGE: &str = "Deleting the Firewall Rule Group";

/// The `FirewallRuleGroup` of a create/get/delete response.
fn rule_group(body: &Value) -> &Value {
    body.get("FirewallRuleGroup")
        .expect("the response carries a FirewallRuleGroup")
}

/// Create a rule group and return its `FirewallRuleGroup`.
async fn create(router: &MultiServiceRouter, request: Value) -> Value {
    rule_group(&ok(router, "CreateFirewallRuleGroup", request).await).clone()
}

/// Read one rule group back over the wire.
async fn read(router: &MultiServiceRouter, id: &str) -> Value {
    rule_group(
        &ok(
            router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
        )
        .await,
    )
    .clone()
}

/// Delete one rule group over the wire and return the `FirewallRuleGroup` it
/// answered with.
async fn delete(router: &MultiServiceRouter, id: &str) -> Value {
    rule_group(
        &ok(
            router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
        )
        .await,
    )
    .clone()
}

/// Wait out the span the clock owes a rule group whose change has not settled,
/// so the settled status is due.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// One row of the delete table: what the rule group was created with.
struct DeleteCase {
    case: &'static str,
    request: Value,
    /// The tags the create carried, which `ListTagsForResource` reports against
    /// the group's ARN until the delete takes the record out.
    tags: usize,
}

/// A delete reports the stored rule group in the step it last reached and
/// removes it: afterwards the reader reports that id missing, a second delete of
/// the same id is a miss too, and the tags the group held are gone with it.
///
/// The table spans what the rule generalises over: the shape of the `Name` the
/// model's pattern `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$` admits (letters, digits
/// beside letters, underscore, hyphen, apostrophe, space, and the published
/// 64-character maximum), a group with tags and one without, and
/// `CreatorRequestId`s of different forms.
#[tokio::test]
async fn a_deleted_rule_group_is_removed_from_state() {
    // 64 characters, the maximum the model's `Name` (length 0..64) allows.
    let longest_name = "g".repeat(64);
    let cases = vec![
        DeleteCase {
            case: "letters only, no Tags",
            request: json!({ "CreatorRequestId": "req-letters", "Name": "blocklist-group" }),
            tags: 0,
        },
        DeleteCase {
            case: "digits beside letters, one tag",
            request: json!({
                "CreatorRequestId": "req-mixed",
                "Name": "group_2-v3",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
            tags: 1,
        },
        DeleteCase {
            case: "spaces and an apostrophe in the name, several tags",
            request: json!({
                "CreatorRequestId": "2026-10-27T19:22:16Z",
                "Name": "ops' rule group",
                "Tags": [
                    { "Key": "team", "Value": "net" },
                    { "Key": "env", "Value": "prod" },
                ],
            }),
            tags: 2,
        },
        DeleteCase {
            case: "one character, an empty tag list",
            request: json!({ "CreatorRequestId": "req-shortest", "Name": "g", "Tags": [] }),
            tags: 0,
        },
        DeleteCase {
            case: "the longest Name the model allows",
            request: json!({ "CreatorRequestId": "req-longest", "Name": longest_name }),
            tags: 0,
        },
    ];

    let router = emulator();
    for DeleteCase {
        case,
        request,
        tags,
    } in cases
    {
        let made = create(&router, request).await;
        let id = string_member(&made, "Id");
        let arn = string_member(&made, "Arn");

        // The tags are on the record the delete removes, so they are reported
        // while the group exists.
        let listed = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceArn": arn.clone() }),
        )
        .await;
        assert_eq!(
            listed
                .get("Tags")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            tags,
            "case {case:?}: the stored group must report the tags its create carried, got {listed}"
        );

        let before = read(&router, &id).await;
        let deleted = delete(&router, &id).await;

        // The delete answers the group it just took out: the record as it
        // stood, not a fresh one.
        for member in [
            "Id",
            "Arn",
            "Name",
            "CreatorRequestId",
            "CreationTime",
            "OwnerId",
            "ShareStatus",
            "RuleCount",
        ] {
            assert_eq!(
                deleted.get(member),
                before.get(member),
                "case {case:?}: {member} must come from the stored group, got {deleted}"
            );
        }
        assert_eq!(
            arn,
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:firewall-rule-group/{id}"),
            "case {case:?}: the Arn must be the stored one, rooted in the signed region, got {made}"
        );
        assert_eq!(
            string_member(&deleted, "OwnerId"),
            ACCOUNT,
            "case {case:?}: OwnerId must be the account that owns the group, got {deleted}"
        );
        let status = string_member(&deleted, "Status");
        assert_eq!(
            status, FIREWALL_RULE_GROUP_STATUS_DELETING,
            "case {case:?}: the delete has been accepted, so the group it answers with is on its way out, got {deleted}"
        );
        assert!(
            FIREWALL_RULE_GROUP_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: Status must be a modelled FirewallRuleGroupStatus, got {status:?}"
        );
        // The status message goes with the step the status names, so the delete
        // states the delete step rather than the one the reads before it stated.
        let message = string_member(&deleted, "StatusMessage");
        assert_eq!(
            message, DELETING_RULE_GROUP_MESSAGE,
            "case {case:?}: the delete must state the step the group stands in, got {deleted}"
        );
        assert_eq!(
            string_member(&before, "StatusMessage"),
            CREATED_RULE_GROUP_MESSAGE,
            "case {case:?}: the reads before the delete state the step the group stood in, got {before}"
        );
        assert!(
            message.len() <= 255,
            "case {case:?}: StatusMessage must satisfy its modelled length 0..255, got {message:?}"
        );
        let created_at = parse_time(case, "CreationTime", &string_member(&deleted, "CreationTime"));
        let modified_at = parse_time(
            case,
            "ModificationTime",
            &string_member(&deleted, "ModificationTime"),
        );
        assert!(
            modified_at >= created_at,
            "case {case:?}: ModificationTime must not precede CreationTime, got {deleted}"
        );

        // Every caller of that id now sees it missing, and the ARN carries no
        // resource — so the tags went with the record.
        for (operation, request) in [
            (
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            ),
            (
                "DeleteFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            ),
            ("ListTagsForResource", json!({ "ResourceArn": arn.clone() })),
        ] {
            expect_error(
                &router,
                operation,
                request,
                case,
                "ResourceNotFoundException",
            )
            .await;
        }
    }
}

/// A delete frees the `CreatorRequestId` the group was made with: a later create
/// reusing that string makes a NEW group rather than resolving to the deleted
/// one (which the retry rule would do while the group still existed).
#[tokio::test]
async fn the_creator_request_id_is_free_again_after_the_delete() {
    let router = emulator();

    for (case, creator_request_id) in [
        ("a plain request id", "req-reused"),
        (
            "a request id that reads like a date/time stamp",
            "2026-10-27T19:22:16Z",
        ),
        ("a request id that is one character", "r"),
    ] {
        let request = json!({ "CreatorRequestId": creator_request_id, "Name": "reused-id" });
        let first = create(&router, request.clone()).await;
        let first_id = string_member(&first, "Id");

        // While the group exists, the same request id resolves to it.
        let retried = create(&router, request.clone()).await;
        assert_eq!(
            string_member(&retried, "Id"),
            first_id,
            "case {case:?}: a retry must resolve to the group that request made, got {retried}"
        );

        delete(&router, &first_id).await;

        let remade = create(&router, request).await;
        let remade_id = string_member(&remade, "Id");
        assert_ne!(
            remade_id, first_id,
            "case {case:?}: the deleted group is gone, so the create must mint a new Id, got {remade}"
        );
        assert_eq!(
            string_member(&read(&router, &remade_id).await, "Id"),
            remade_id,
            "case {case:?}: the new group must be readable under its own id"
        );
    }
}

/// An id the store holds no rule group under is reported with the
/// `ResourceNotFoundException` the operation models — never an empty success,
/// and never a group it invented. The rows walk the forms a wrong id takes: one
/// nothing minted, another resource's id, the shortest `ResourceId` the model
/// allows, the longest, and a near miss of a stored id.
#[tokio::test]
async fn an_unknown_rule_group_id_is_reported_as_missing() {
    let router = emulator();
    let stored = string_member(
        &create(
            &router,
            json!({ "CreatorRequestId": "req-kept", "Name": "kept" }),
        )
        .await,
        "Id",
    );

    for (case, id) in [
        (
            "an id nothing minted",
            "rslvr-frg-0123456789abcdef".to_string(),
        ),
        (
            "a firewall domain list id",
            "rslvr-fdl-0123456789abcdef".to_string(),
        ),
        ("one character, the shortest ResourceId", "x".to_string()),
        (
            "64 characters, the longest ResourceId",
            format!("rslvr-frg-{}", "f".repeat(54)),
        ),
        (
            "a near miss of a stored id",
            stored[..stored.len() - 1].to_string(),
        ),
    ] {
        let (status, body) = call(
            &router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an id nothing is stored under must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {case:?}: the refusal must name ResourceNotFoundException, got {body}"
        );
        // The refusal names the resource the caller asked for, so a caller can
        // tell which id was not found, and the type of resource it looked for.
        let message = string_member(&body, "Message") + &string_member(&body, "message");
        assert!(
            message.contains(&id),
            "case {case:?}: the refusal must name the id that was not found, got {body}"
        );
        assert_eq!(
            string_member(&body, "ResourceType"),
            "FirewallRuleGroup",
            "case {case:?}: the refusal must name the resource type, got {body}"
        );
    }

    // None of the refusals touched the stored group.
    assert_eq!(
        string_member(&read(&router, &stored).await, "Id"),
        stored,
        "case \"the stored group after the refusals\": it must still be readable"
    );
}

/// A rule group whose last change has not settled yet has a state transition in
/// flight, so the delete is refused with the `ConflictException` the operation
/// models and the record is left exactly as it was. Once the clock has made the
/// settled status due, the very same delete is served.
///
/// The rows are the two transient `FirewallRuleGroupStatus` values the model
/// declares beside the settled one
/// (`smithy_query(query="shape",shape="FirewallRuleGroupStatus")`), put on a
/// stored group through the store the service reads, because the operations that
/// move a group into one of them are not implemented yet.
#[tokio::test]
async fn a_rule_group_whose_change_has_not_settled_cannot_be_deleted_yet() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    for (case, transient) in FIREWALL_RULE_GROUP_STATUS_VALUES
        .iter()
        .filter(|status| **status != FIREWALL_RULE_GROUP_STATUS_COMPLETE)
        .map(|status| (format!("a group reported {status}"), *status))
    {
        let made = create(
            &router,
            json!({
                "CreatorRequestId": format!("req-{transient}"),
                "Name": format!("settling-{transient}"),
            }),
        )
        .await;
        let id = string_member(&made, "Id");

        {
            let state = service_state(&*engine)
                .expect("the store is reachable from the engine the service is mounted on");
            let mut store = state.lock();
            let record = store
                .firewall_rule_groups
                .get_mut(&id)
                .expect("the create wrote a record for the group it answered with");
            record.status = transient.to_string();
            record.settled_at = Some(Instant::now() + PROVISIONING_DELAY);
        }

        let reported = read(&router, &id).await;
        assert_eq!(
            string_member(&reported, "Status"),
            transient,
            "case {case:?}: the change has not settled yet, got {reported}"
        );

        expect_error(
            &router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id.clone() }),
            &case,
            "ConflictException",
        )
        .await;

        // The refusal states what the service states for this error class.
        let (_, refusal) = call(
            &router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id.clone() }),
        )
        .await;
        assert_eq!(
            string_member(&refusal, "Message") + &string_member(&refusal, "message"),
            STATE_TRANSITION_CONFLICT,
            "case {case:?}: the refusal must state the invalid state transition, got {refusal}"
        );

        // The refusal wrote nothing: the group is still there, under its own
        // name, and its id is still the one the create minted.
        let kept = read(&router, &id).await;
        assert_eq!(
            string_member(&kept, "Name"),
            format!("settling-{transient}"),
            "case {case:?}: the refused delete must leave the group as it was, got {kept}"
        );

        settle().await;

        let deleted = delete(&router, &id).await;
        assert_eq!(
            string_member(&deleted, "Status"),
            FIREWALL_RULE_GROUP_STATUS_DELETING,
            "case {case:?}: once the change has settled the delete is served, got {deleted}"
        );
        assert_eq!(
            string_member(&deleted, "StatusMessage"),
            DELETING_RULE_GROUP_MESSAGE,
            "case {case:?}: the served delete states the step the group stands in, got {deleted}"
        );
        expect_error(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
            &case,
            "ResourceNotFoundException",
        )
        .await;
    }
}

/// A delete touches ONE record: the groups beside it keep their own members and
/// stay readable, and the store keeps exactly the records the deletes left.
#[tokio::test]
async fn a_delete_leaves_the_groups_beside_it_alone() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    let mut ids = Vec::new();
    for index in 0..6 {
        let made = create(
            &router,
            json!({
                "CreatorRequestId": format!("req-neighbour-{index}"),
                "Name": format!("neighbour-{index}"),
            }),
        )
        .await;
        ids.push(string_member(&made, "Id"));
    }

    // Delete every second group, so a kept group sits on each side of a deleted
    // one.
    let deleted: Vec<String> = ids
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(_, id)| id.clone())
        .collect();
    let kept = ids.len() - deleted.len();

    for id in &deleted {
        delete(&router, id).await;
    }

    for (index, id) in ids.iter().enumerate() {
        let case = format!("group {index}");
        if deleted.contains(id) {
            expect_error(
                &router,
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id }),
                &case,
                "ResourceNotFoundException",
            )
            .await;
        } else {
            let still_there = read(&router, id).await;
            assert_eq!(
                string_member(&still_there, "Name"),
                format!("neighbour-{index}"),
                "case {case:?}: a group beside a deleted one must keep its own Name, got {still_there}"
            );
        }
    }

    let stored = service_state(&*engine)
        .expect("the store is reachable from the engine the service is mounted on")
        .lock()
        .firewall_rule_groups
        .len();
    assert_eq!(
        stored, kept,
        "case \"the store after the deletes\": it must hold exactly the groups that were kept, got {stored}"
    );
}
