// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeletePeering`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`, the core-network-with-edges helper in
//! `crate::attachment_lifecycle`, and the peering helpers in
//! `crate::transit_gateway_peering`).
//!
//! The rule under test: a delete of a peering REMOVES the one stored record the `PeeringId`
//! names and reports exactly that record in `DELETING`, so every other reader of the store —
//! the peering's own reader, the network-resource inventory and its tags — stops reporting it
//! while every peering the delete did not name is left where it was; a `PeeringId` naming no
//! stored peering, never created or already deleted, is refused with
//! `ResourceNotFoundException` (HTTP 404) and removes nothing; and a peering an attachment was
//! created over cannot be deleted while that attachment is stored, which is this service's
//! dependency refusal (`ValidationException`, HTTP 400) and stays until the caller removes the
//! dependent.
//!
//! The table spans that class: rows differing in the Region (and so the edge location) of the
//! peered transit gateway, in the ARN partition, in whether the gateway belongs to another
//! account, in the width of the gateway id, in how many tags the create carried and in whether
//! it carried a client token; and rows differing in which side of the transition the clock owes
//! the peering is on when the delete arrives. Nothing reaches into `crate::state`: every
//! peering is created, read back and deleted through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::core_network_at;
use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::{list_tags, tag};
use crate::transit_gateway_peering::{
    create_peering, own_account, peering_count, peering_id, transit_gateway_arn, PEERING,
};

/// The id prefix and ARN resource type of an attachment: the model's own
/// `^attachment-([0-9a-f]{8,17})$` (`smithy_query shape=AttachmentId`).
const ATTACHMENT: &str = "attachment";

/// The edges the core networks under test configure, one per Region: a peering sits at the
/// edge of the Region its transit gateway is in, so a row's Region is a real choice.
const EDGES: [&str; 3] = ["us-west-2", "eu-west-1", "ap-southeast-2"];

/// One row of the table: the peering one `CreateTransitGatewayPeering` asks for and the
/// delete then removes.
struct DeleteCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The Region the peered transit gateway is in, which is the edge location the peering
    /// sits at. Every one of these is an edge of the core network the test builds.
    region: &'static str,
    /// The ARN partition the gateway's ARN carries.
    partition: &'static str,
    /// The account the gateway belongs to, or `None` for the emulator's own account.
    account: Option<&'static str>,
    /// How many hex digits the gateway id carries (8 to 17, the width the modelled EC2 ARN
    /// patterns allow).
    digits: usize,
    /// The tags the create carries, which are the tags the delete has to report back.
    tags: &'static [(&'static str, &'static str)],
    /// The `ClientToken` the create carries, or `None` for a create naming none.
    client_token: Option<&'static str>,
}

/// The table: rows differing along everything the rule generalises over — the Region and so
/// the edge location, the partition, whose account the gateway is in, the width of its id, the
/// tags and the client token.
const DELETE_CASES: [DeleteCase; 5] = [
    DeleteCase {
        name: "a gateway of this account, the shortest id the pattern allows, no tags",
        region: "us-west-2",
        partition: "aws",
        account: None,
        digits: 8,
        tags: &[],
        client_token: None,
    },
    DeleteCase {
        name: "a gateway at another edge, the longest id the pattern allows, one tag",
        region: "eu-west-1",
        partition: "aws",
        account: None,
        digits: 17,
        tags: &[("Name", "dublin")],
        client_token: Some("delete-token-eu"),
    },
    DeleteCase {
        name: "a gateway of another account, two tags",
        region: "ap-southeast-2",
        partition: "aws",
        account: Some("210987654321"),
        digits: 12,
        tags: &[("Name", "sydney"), ("Tier", "edge")],
        client_token: None,
    },
    DeleteCase {
        name: "a gateway in another partition",
        region: "us-west-2",
        partition: "aws-us-gov",
        account: None,
        digits: 9,
        tags: &[],
        client_token: Some("delete-token-gov"),
    },
    DeleteCase {
        name: "a second gateway at the first edge, carrying a tag and a token",
        region: "us-west-2",
        partition: "aws",
        account: None,
        digits: 17,
        tags: &[("Stage", "prod")],
        client_token: Some("delete-token-second"),
    },
];

/// The gateway ARN the row `case` peers with. `index` keeps the gateways of two rows (and of
/// two subjects) apart, since one gateway is peered once.
fn case_gateway(case: &DeleteCase, index: usize) -> String {
    transit_gateway_arn(
        case.partition,
        case.region,
        case.account.unwrap_or(&own_account()),
        index,
        case.digits,
    )
}

/// The `CreateTransitGatewayPeering` request body the row `case` asks for.
fn request_of(case: &DeleteCase, core_network: &str, index: usize) -> Value {
    let mut body = json!({
        "CoreNetworkId": core_network,
        "TransitGatewayArn": case_gateway(case, index),
    });
    let object = body
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", case.name));
    if !case.tags.is_empty() {
        let tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(key, value)| tag(key, value))
            .collect();
        object.insert("Tags".to_string(), json!(tags));
    }
    if let Some(token) = case.client_token {
        object.insert("ClientToken".to_string(), json!(token));
    }
    body
}

/// `DeletePeering`.
async fn delete_peering(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator.delete(&format!("/peerings/{id}")).await
}

/// `GetTransitGatewayPeering`, status and body as answered — a deleted peering is a refusal,
/// so this suite cannot use the asserting reader the create/read suite shares.
async fn get_peering(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator
        .get(&format!("/transit-gateway-peerings/{id}"))
        .await
}

/// Create the peering of the row `case` on `core_network`, asserting the create succeeded, and
/// answer its id with the `Peering` envelope the create reported — the record the delete has to
/// report back, in `DELETING` rather than the state the create reported.
async fn create(
    emulator: &Emulator,
    core_network: &str,
    case: &DeleteCase,
    index: usize,
) -> (String, Value) {
    let name = case.name;
    let (status, body) = create_peering(emulator, request_of(case, core_network, index)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateTransitGatewayPeering expected 200, got {status} with {body}"
    );
    let wrapper = body["TransitGatewayPeering"].clone();
    let id = peering_id(name, &wrapper);
    assert!(
        is_minted_id(&id, PEERING),
        "case {name:?}: the id is minted by the service, got {id:?}"
    );
    (id, wrapper["Peering"].clone())
}

/// Assert `body` is the modelled not-found for the peering `id`: HTTP 404 with the error code,
/// the resource type and the id the request asked for.
fn assert_not_found(case: &str, operation: &str, status: u16, body: &Value, id: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: {operation} on an id naming no stored peering expected 404, got {status} \
         with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: {operation} expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!(PEERING),
        "case {case:?}: {operation} must name the resource type, got {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(id),
        "case {case:?}: {operation} must name the id asked for, got {body}"
    );
}

#[tokio::test]
async fn a_delete_removes_the_peering_it_names_reports_it_deleting_and_leaves_the_others() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "peering deletes", &EDGES).await;

    let mut created: Vec<(&DeleteCase, String, Value)> = Vec::new();
    for (index, case) in DELETE_CASES.iter().enumerate() {
        let (id, at_create) = create(&emulator, &core_network, case, index).await;
        created.push((case, id, at_create));
    }

    assert_eq!(
        peering_count(&emulator, &core_network).await,
        created.len(),
        "the inventory must count every stored peering before any delete"
    );

    for (position, (case, id, at_create)) in created.iter().enumerate() {
        let name = case.name;
        let arn = expected_arn(PEERING, id);

        let (status, deleted) = delete_peering(&emulator, id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeletePeering expected 200, got {status} with {deleted}"
        );
        // The delete reports the record it removed: the very peering the create stored, with
        // the state of one on its way out rather than the state the clock would compute.
        let mut expected = at_create.clone();
        expected["State"] = json!("DELETING");
        assert_eq!(
            deleted["Peering"], expected,
            "case {name:?}: the delete must report the stored peering in DELETING, got {deleted}"
        );
        // Named explicitly because a peering's tags are what the delete takes with it.
        assert_eq!(
            deleted["Peering"]["Tags"], at_create["Tags"],
            "case {name:?}: the delete must report the tags the peering held, got {deleted}"
        );
        // And the gateway it peered: the removed record is reported in full rather than as a
        // shell carrying only an id.
        assert_eq!(
            deleted["Peering"]["ResourceArn"],
            json!(case_gateway(case, position)),
            "case {name:?}: the delete must report the gateway the peering peered, got {deleted}"
        );

        // The record is really gone, not re-labelled: the peering's own reader, a second
        // delete and its tags all answer not-found afterwards.
        let (status, body) = get_peering(&emulator, id).await;
        assert_not_found(name, "GetTransitGatewayPeering", status, &body, id);
        let (status, body) = delete_peering(&emulator, id).await;
        assert_not_found(name, "a second DeletePeering", status, &body, id);
        let (status, body) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 404,
            "case {name:?}: ListTagsForResource on a deleted peering expected 404, got {status} \
             with {body}"
        );

        // Every peering the delete did not name is where it was: readable and counted.
        for (other_case, other_id, _) in created.iter().skip(position + 1) {
            let (status, body) = get_peering(&emulator, other_id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: deleting it must leave the peering of {:?} readable, got {status} \
                 with {body}",
                other_case.name
            );
        }
        assert_eq!(
            peering_count(&emulator, &core_network).await,
            created.len() - position - 1,
            "case {name:?}: the inventory must count only the peerings still stored"
        );
    }
}

#[tokio::test]
async fn a_delete_of_an_id_naming_no_stored_peering_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "peering delete refusals", &EDGES).await;

    // One stored peering, so a refused delete has something it could have removed.
    let (stored, _) = create(&emulator, &core_network, &DELETE_CASES[0], 10).await;

    // And one that WAS stored and is already gone, so the rows cover both ways an id can name
    // nothing: never minted, and minted and then deleted.
    let (gone, _) = create(&emulator, &core_network, &DELETE_CASES[1], 11).await;
    let (status, body) = delete_peering(&emulator, &gone).await;
    assert_eq!(
        status, 200,
        "setup DeletePeering expected 200, got {status} with {body}"
    );

    // Rows differing in WHY the id names nothing: never minted (at each end of the id width
    // the model's `^peering-([0-9a-f]{8,17})$` allows), and already deleted.
    for (row, id) in [
        ("an id that was never minted", "peering-0000000000000ffff"),
        ("the shortest id the pattern allows", "peering-0000000a"),
        ("an id that was deleted already", gone.as_str()),
    ] {
        let (status, body) = delete_peering(&emulator, id).await;
        assert_not_found(row, "DeletePeering", status, &body, id);
    }

    // Nothing the refusals named was removed, and the peering that IS stored is untouched.
    let (status, body) = get_peering(&emulator, &stored).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the stored peering readable, got {status} with {body}"
    );
    assert_eq!(
        peering_count(&emulator, &core_network).await,
        1,
        "a refused delete must leave the inventory as it was"
    );
}

#[tokio::test]
async fn a_peering_is_deleted_on_either_side_of_the_transition_the_clock_owes() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "peering delete timing", &EDGES).await;

    // Each row is deleted on a different side of the transition the create owes: straight
    // after the create, while the peering is still CREATING, and once it has fallen due and
    // the peering is AVAILABLE. The delete is about what is STORED, so both answer the removed
    // record in DELETING.
    for (row, case, index, settle) in [
        ("deleted while still CREATING", &DELETE_CASES[0], 20, false),
        ("deleted once AVAILABLE", &DELETE_CASES[2], 21, true),
    ] {
        let name = case.name;
        let (id, at_create) = create(&emulator, &core_network, case, index).await;
        assert_eq!(
            at_create["State"],
            json!("CREATING"),
            "case {row:?} / {name:?}: a created peering waits in CREATING, got {at_create}"
        );
        if settle {
            tokio::time::sleep(PAST_SETTLE).await;
            let (status, read) = get_peering(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {row:?}: GetTransitGatewayPeering expected 200, got {status} with {read}"
            );
            assert_eq!(
                read["TransitGatewayPeering"]["Peering"]["State"],
                json!("AVAILABLE"),
                "case {row:?}: past the owed transition the peering is AVAILABLE, got {read}"
            );
        }

        let (status, deleted) = delete_peering(&emulator, &id).await;
        assert_eq!(
            status, 200,
            "case {row:?}: DeletePeering expected 200, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["Peering"]["State"],
            json!("DELETING"),
            "case {row:?}: the removed record is reported in DELETING, got {deleted}"
        );
        assert_eq!(
            deleted["Peering"]["PeeringId"],
            json!(id),
            "case {row:?}: the delete must answer the peering it removed, got {deleted}"
        );
        let (status, body) = get_peering(&emulator, &id).await;
        assert_not_found(row, "GetTransitGatewayPeering", status, &body, &id);
    }

    assert_eq!(
        peering_count(&emulator, &core_network).await,
        0,
        "every peering created here was deleted, so the inventory counts none"
    );
}

/// A transit gateway route table ARN in `region`, in the documented form of
/// `TransitGatewayRouteTableArn` — `arn:aws:ec2:us-west-2:123456789012:
/// transit-gateway-route-table/tgw-rtb-9876543210123456` (`smithy_query query="doc"
/// shape="TransitGatewayRouteTableAttachment" member="TransitGatewayRouteTableArn"`).
fn route_table_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:transit-gateway-route-table/tgw-rtb-{index:017x}",
        own_account()
    )
}

#[tokio::test]
async fn a_peering_an_attachment_was_created_over_is_not_deleted_until_that_attachment_is_gone() {
    let emulator = Emulator::new();
    let (_, core_network) = core_network_at(&emulator, "peering dependents", &EDGES).await;

    // One row per edge of the core network, so the refusal is about the dependent the peering
    // carries rather than about one Region.
    for (index, region) in EDGES.iter().enumerate() {
        let case = DeleteCase {
            name: "a peering carrying a route table attachment",
            region,
            partition: "aws",
            account: None,
            digits: 17,
            tags: &[],
            client_token: None,
        };
        let position = 30 + index;
        let (peering, _) = create(&emulator, &core_network, &case, position).await;

        let (status, attached) = emulator
            .post(
                "/transit-gateway-route-table-attachments",
                json!({
                    "PeeringId": peering,
                    "TransitGatewayRouteTableArn": route_table_arn(region, position),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreateTransitGatewayRouteTableAttachment expected 200, got {status} \
             with {attached}"
        );
        let attachment = attached["TransitGatewayRouteTableAttachment"]["Attachment"]
            ["AttachmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: no AttachmentId in {attached}"))
            .to_string();
        assert!(
            is_minted_id(&attachment, ATTACHMENT),
            "case {region:?}: the attachment id is minted by the service, got {attachment:?}"
        );

        // The delete is refused while the attachment it would orphan is stored: a validation
        // failure against the stored resource tree, naming the peering being deleted and the
        // dependent kind that blocks it, blaming no input field.
        let (status, refused) = delete_peering(&emulator, &peering).await;
        assert_eq!(
            status, 400,
            "case {region:?}: a dependency refusal is HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {region:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Reason"],
            json!("Other"),
            "case {region:?}: no input field is at fault, so the reason is the modelled Other, \
             got {refused}"
        );
        assert_eq!(
            refused["Fields"],
            Value::Null,
            "case {region:?}: a dependency refusal blames no field, got {refused}"
        );
        let message = refused["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: no Message in {refused}"));
        assert!(
            message.contains("Peering"),
            "case {region:?}: expected the message to name the peering being deleted, got \
             {message:?}"
        );
        assert!(
            message.contains(ATTACHMENT),
            "case {region:?}: expected the message to name the blocking attachment, got \
             {message:?}"
        );

        // The refusal changed nothing: both records are still stored.
        let (status, read) = get_peering(&emulator, &peering).await;
        assert_eq!(
            status, 200,
            "case {region:?}: a refused delete must leave the peering readable, got {status} with \
             {read}"
        );
        let (status, read) = emulator
            .get(&format!(
                "/transit-gateway-route-table-attachments/{attachment}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: a refused delete must leave the attachment readable, got {status} \
             with {read}"
        );

        // Once the dependent is gone the identical call succeeds, so the refusal tracked stored
        // state rather than the shape of the request.
        let (status, removed) = emulator.delete(&format!("/attachments/{attachment}")).await;
        assert_eq!(
            status, 200,
            "case {region:?}: DeleteAttachment expected 200, got {status} with {removed}"
        );
        let (status, deleted) = delete_peering(&emulator, &peering).await;
        assert_eq!(
            status, 200,
            "case {region:?}: with the attachment gone the same delete must succeed, got {status} \
             with {deleted}"
        );
        assert_eq!(
            deleted["Peering"]["State"],
            json!("DELETING"),
            "case {region:?}: the removed record is reported in DELETING, got {deleted}"
        );
        let (status, body) = get_peering(&emulator, &peering).await;
        assert_not_found(region, "GetTransitGatewayPeering", status, &body, &peering);
    }

    assert_eq!(
        peering_count(&emulator, &core_network).await,
        0,
        "every peering created here was deleted, so the inventory counts none"
    );
}
