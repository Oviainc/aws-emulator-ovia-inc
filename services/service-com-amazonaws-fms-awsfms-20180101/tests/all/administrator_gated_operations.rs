// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the administrator gate on Firewall Manager's
//! organisation-scoped operations, and for what each of those operations does
//! once an administrator IS associated.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: an organisation-scoped Firewall Manager
//! operation decides authorization FIRST, so with no Firewall Manager default
//! administrator association in the account/Region it answers HTTP 400
//! `AccessDeniedException` before it looks up any identifier, validates any
//! payload or writes any state — and only with an administrator associated does
//! it act on the store. The table in [`gated_calls`] is the class that rule
//! ranges over: one row per gated operation, the rows differing in payload form
//! (no members at all, an id, ARNs, enum values, a malformed ARN, a bogus
//! resource type), each asserted in more than one Region.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{
    App, AppsListData, DiscoveredResource, ProtocolsListData,
};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness lives in one place for this test target: `policy_lifecycle`
// owns it, `member_accounts` reuses it, and so does this suite — so the two
// cannot drift in how a request is signed or an error envelope is read.
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A second Region, to assert the gate's answer does not depend on which Region
/// the request addresses.
const OTHER_REGION: &str = "eu-central-1";

/// A `ListId` (the model's `com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names nothing.
const UNKNOWN_LIST_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A second well-formed `ListId` that names nothing.
const OTHER_UNKNOWN_LIST_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A `Base62Id` (the model's `com.amazonaws.fms#Base62Id`: 22 characters
/// matching `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "0123456789AbCdEfGhIjKl";

/// The `message` member of an awsJson1_1 error envelope, under either casing: the
/// modelled error shapes of this service spell it `Message`, the synthetic
/// authorization refusal `message`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Make this account the Firewall Manager default administrator, which is what
/// opens the gate.
///
/// `pub(crate)` so a sibling suite in this test target opens the gate the same
/// way rather than growing a second copy of it.
pub(crate) async fn associate_admin(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "AssociateAdminAccount",
        &json!({ "AdminAccount": ACCOUNT }),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateAdminAccount must answer 200, got {status} with {body}"
    );
}

/// Read or write the service's store the way a handler does — used to seed the
/// resources whose own writers (`PutAppsList`, `AssociateThirdPartyFirewall`,
/// resource discovery) are still stubs tracked by their own issues, and the
/// managed lists no API creates.
///
/// `pub(crate)` so a sibling suite in this test target reaches the store through
/// the same seam.
pub(crate) fn with_store<R>(
    engine: &Arc<engine::InMemoryEngine>,
    body: impl FnOnce(&mut state::ComAmazonawsFmsAwsfms20180101State) -> R,
) -> R {
    let handle = state::service_state(&**engine).expect("the Firewall Manager store");
    let mut store = handle.lock();
    body(&mut store)
}

/// One stored protocols list, as `PutProtocolsList` writes it. `pub(crate)` so
/// the `protocols_list_lifecycle` suite seeds a Firewall Manager MANAGED list —
/// which no API creates — through the same helper.
pub(crate) fn seed_protocols_list(
    store: &mut state::ComAmazonawsFmsAwsfms20180101State,
    list_id: &str,
    list_name: &str,
    protocols: &[&str],
    default_list: bool,
) {
    let now = SystemTime::now();
    store.protocols_lists.insert(
        list_id.to_string(),
        state::ProtocolsListRecord {
            list_id: list_id.to_string(),
            list_arn: state::protocols_list_arn(REGION, list_id),
            update_token: state::mint_identifier(),
            default_list,
            list: ProtocolsListData {
                list_name: list_name.to_string(),
                protocols_list: protocols.iter().map(|p| (*p).to_string()).collect(),
                ..ProtocolsListData::default()
            },
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
        },
    );
}

/// One stored applications list, as `PutAppsList` will write it: the record
/// `GetAppsList` and `DeleteAppsList` read, seeded directly because that
/// writer is still a stub tracked by its own issue.
///
/// The ARN carries `region`, so a suite can assert a reader reports the ARN of
/// the Region the record was stored under; `default_list` picks the namespace
/// (a Firewall Manager MANAGED list, which no API creates, or a customer one).
/// The seeded record is handed back so a caller can assert against the
/// service-owned members it minted.
///
/// `pub(crate)` so a sibling suite in this test target seeds an applications
/// list through the same seam rather than growing a second copy of it.
pub(crate) fn seed_apps_list(
    store: &mut state::ComAmazonawsFmsAwsfms20180101State,
    region: &str,
    list_id: &str,
    list_name: &str,
    apps: &[App],
    default_list: bool,
) -> state::AppsListRecord {
    let now = SystemTime::now();
    let record = state::AppsListRecord {
        list_id: list_id.to_string(),
        list_arn: state::apps_list_arn(region, list_id),
        update_token: state::mint_identifier(),
        default_list,
        list: AppsListData {
            list_name: list_name.to_string(),
            apps_list: apps.to_vec(),
            ..AppsListData::default()
        },
        tags: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    store
        .apps_lists
        .insert(list_id.to_string(), record.clone());
    record
}

/// One row of the gated-operation table: the operation, and a request for it.
struct Gated {
    operation: &'static str,
    body: Value,
}

/// Every administrator-gated operation this change implements, with a request
/// whose SHAPE differs from row to row: a request with no members at all, one
/// naming a nonexistent id, one carrying ARNs, one carrying a required enum, one
/// carrying a malformed ARN beside a well-formed one, and one carrying a
/// resource type no registry declares.
fn gated_calls() -> Vec<Gated> {
    vec![
        Gated {
            operation: "BatchAssociateResource",
            body: json!({
                "ResourceSetIdentifier": UNKNOWN_RESOURCE_SET_ID,
                "Items": [
                    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/x"),
                    "this-is-not-an-arn-at-all",
                ],
            }),
        },
        Gated {
            operation: "DeleteAppsList",
            body: json!({ "ListId": UNKNOWN_LIST_ID }),
        },
        Gated {
            operation: "DeleteProtocolsList",
            body: json!({ "ListId": OTHER_UNKNOWN_LIST_ID }),
        },
        Gated {
            operation: "GetAppsList",
            body: json!({ "ListId": OTHER_UNKNOWN_LIST_ID }),
        },
        Gated {
            operation: "GetProtocolsList",
            body: json!({ "ListId": UNKNOWN_LIST_ID, "DefaultList": true }),
        },
        Gated {
            operation: "ListProtocolsLists",
            body: json!({ "MaxResults": 100 }),
        },
        Gated {
            operation: "PutProtocolsList",
            body: json!({
                "ProtocolsList": {
                    "ListName": "gated",
                    "ProtocolsList": ["TCP", "UDP"],
                },
                "TagList": [{ "Key": "gated", "Value": "yes" }],
            }),
        },
        Gated {
            operation: "GetNotificationChannel",
            body: json!({}),
        },
        Gated {
            operation: "PutNotificationChannel",
            body: json!({
                "SnsTopicArn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:fms-alerts"),
                "SnsRoleName": format!("arn:aws:iam::{ACCOUNT}:role/fms-sns"),
            }),
        },
        Gated {
            operation: "ListDiscoveredResources",
            body: json!({
                "MemberAccountIds": [ACCOUNT],
                "ResourceType": "AWS::EC2::Instance",
            }),
        },
        Gated {
            operation: "PutResourceSet",
            body: json!({
                "ResourceSet": {
                    "Name": "gated",
                    "ResourceTypeList": ["AWS::Bogus::NotARealType"],
                },
            }),
        },
        Gated {
            operation: "DisassociateThirdPartyFirewall",
            body: json!({ "ThirdPartyFirewall": "PALO_ALTO_NETWORKS_CLOUD_NGFW" }),
        },
        Gated {
            operation: "GetThirdPartyFirewallAssociationStatus",
            body: json!({ "ThirdPartyFirewall": "FORTIGATE_CLOUD_NATIVE_FIREWALL" }),
        },
        Gated {
            operation: "GetResourceSet",
            body: json!({ "Identifier": UNKNOWN_RESOURCE_SET_ID }),
        },
        Gated {
            operation: "DeleteResourceSet",
            body: json!({ "Identifier": UNKNOWN_RESOURCE_SET_ID }),
        },
        Gated {
            operation: "ListResourceSets",
            body: json!({ "MaxResults": 5 }),
        },
    ]
}

/// With no administrator association, EVERY gated operation answers HTTP 400
/// `AccessDeniedException` — whatever its payload and whichever Region the
/// request is signed for — and none of them is answered with the scaffold's 501
/// or with a resource-specific error.
#[tokio::test]
async fn no_administrator_association_refuses_every_gated_operation_with_access_denied() {
    for region in [REGION, OTHER_REGION] {
        let (router, _engine) = emulator_with_engine();

        // The precondition, read through an implemented operation rather than
        // assumed: nothing is associated, which GetAdminAccount reports as
        // ResourceNotFoundException.
        let (status, body) = call_in(&router, "GetAdminAccount", &json!({}), region).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "region {region}: expected no administrator association, got {status} with {body}"
        );

        for case in gated_calls() {
            let (status, body) = call_in(&router, case.operation, &case.body, region).await;
            assert_eq!(
                status, 400,
                "case {:?} in {region}: expected HTTP 400, got {status} with {body}",
                case.operation
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {:?} in {region}: expected AccessDeniedException, got {body}",
                case.operation
            );
            assert!(
                !error_message(&body).is_empty() && error_message(&body) != "<no message>",
                "case {:?} in {region}: the refusal must carry a message, got {body}",
                case.operation
            );
            assert!(
                error_message(&body).contains(ACCOUNT) && error_message(&body).contains(region),
                "case {:?} in {region}: the refusal's message is built from the request, so it \
                 must name the account and Region the administrator was looked for in, got {}",
                case.operation,
                error_message(&body)
            );
        }
    }
}

/// A refused call writes NOTHING: after the whole gated table has been refused,
/// associating an administrator reveals an empty store — no notification
/// channel, no resource set, no list, no vendor association.
#[tokio::test]
async fn a_refused_gated_call_leaves_no_state_behind() {
    let (router, engine) = emulator_with_engine();
    for case in gated_calls() {
        let (status, _) = call(&router, case.operation, &case.body).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the gated call to be refused",
            case.operation
        );
    }

    associate_admin(&router).await;

    let (status, body) = call(&router, "GetNotificationChannel", &json!({})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "the refused PutNotificationChannel must have stored no channel, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "GetResourceSet",
        &json!({ "Identifier": UNKNOWN_RESOURCE_SET_ID }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "the refused PutResourceSet must have created nothing, got {status} with {body}"
    );
    with_store(&engine, |store| {
        assert!(
            store.resource_sets.is_empty()
                && store.apps_lists.is_empty()
                && store.protocols_lists.is_empty()
                && store.notification_channel.is_none()
                && store.third_party_firewalls.is_empty(),
            "a refused gated call must write nothing, store holds {store:?}"
        );
    });
}

/// With an administrator associated, `PutNotificationChannel` stores the channel
/// and `GetNotificationChannel` reads back exactly what was sent — for every row
/// of a table whose ARNs differ in service, in Region segment and in length —
/// and there is ONE channel, so the last write wins.
#[tokio::test]
async fn the_notification_channel_round_trips_and_is_single() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    // Before any write there is no channel resource to report.
    let (status, body) = call(&router, "GetNotificationChannel", &json!({})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "an unconfigured channel must be ResourceNotFoundException, got {status} with {body}"
    );

    // Rows differ in topic name length, in the Region segment of the topic ARN
    // (a channel may point at a topic in another Region) and in role path.
    let cases = [
        (
            format!("arn:aws:sns:{REGION}:{ACCOUNT}:fms-alerts"),
            format!("arn:aws:iam::{ACCOUNT}:role/aws-service-role/fms.amazonaws.com/AWSServiceRoleForFMS"),
        ),
        (
            format!("arn:aws:sns:{OTHER_REGION}:{ACCOUNT}:a"),
            format!("arn:aws:iam::{ACCOUNT}:role/fms"),
        ),
        (
            format!("arn:aws:sns:{REGION}:{ACCOUNT}:fms.notifications-with.dots_and-dashes"),
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/fms-sns-role"),
        ),
    ];

    for (topic, role) in &cases {
        let (status, body) = call(
            &router,
            "PutNotificationChannel",
            &json!({ "SnsTopicArn": topic, "SnsRoleName": role }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {topic:?}: expected PutNotificationChannel to answer 200, got {status} with \
             {body}"
        );

        let (status, got) = call(&router, "GetNotificationChannel", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {topic:?}: expected GetNotificationChannel to answer 200, got {status} with \
             {got}"
        );
        assert_eq!(
            got["SnsTopicArn"].as_str(),
            Some(topic.as_str()),
            "case {topic:?}: the channel must report the topic that was put, got {got}"
        );
        assert_eq!(
            got["SnsRoleName"].as_str(),
            Some(role.as_str()),
            "case {topic:?}: the channel must report the role that was put, got {got}"
        );
    }

    // One channel per account/Region: the last write is what a reader sees.
    let (_, got) = call(&router, "GetNotificationChannel", &json!({})).await;
    let (last_topic, last_role) = cases.last().expect("the table has rows");
    assert_eq!(
        (got["SnsTopicArn"].as_str(), got["SnsRoleName"].as_str()),
        (Some(last_topic.as_str()), Some(last_role.as_str())),
        "the channel is single, so the last write must win, got {got}"
    );
}

/// One row of the resource-set table.
struct ResourceSetCase {
    name: &'static str,
    resource_set: Value,
    tags: Value,
}

/// The resource-set class the rules are asserted over: the rows differ in which
/// optional members are present, in how many resource types the set holds, in
/// description length and in tag count.
fn resource_set_cases() -> Vec<ResourceSetCase> {
    vec![
        ResourceSetCase {
            // The minimum a resource set can be: every required member, nothing else.
            name: "minimal",
            resource_set: json!({
                "Name": "minimal-set",
                "ResourceTypeList": ["AWS::NetworkFirewall::Firewall"],
            }),
            tags: json!([]),
        },
        ResourceSetCase {
            name: "described-with-three-types",
            resource_set: json!({
                "Name": "set.with-three_types",
                "Description": "resources imported into a Firewall Manager policy",
                "ResourceTypeList": [
                    "AWS::EC2::Instance",
                    "AWS::EC2::VPC",
                    "AWS::ElasticLoadBalancingV2::LoadBalancer",
                ],
            }),
            tags: json!([{ "Key": "team", "Value": "security" }]),
        },
        ResourceSetCase {
            name: "many-tags",
            resource_set: json!({
                "Name": "tagged-set",
                "ResourceTypeList": ["AWS::EC2::Instance"],
            }),
            tags: json!([
                { "Key": "a", "Value": "1" },
                { "Key": "b", "Value": "2" },
                { "Key": "c", "Value": "3" },
            ]),
        },
    ]
}

/// `PutResourceSet` mints the `Base62Id`, the `UpdateToken` and the ARN, defaults
/// the status, and `GetResourceSet` reads back exactly what was stored — for
/// every row of the table.
#[tokio::test]
async fn put_resource_set_mints_identifiers_and_get_resource_set_reads_it_back() {
    let statuses = service_com_amazonaws_fms_awsfms_20180101::ops::types::RESOURCE_SET_STATUS_VALUES;
    for case in resource_set_cases() {
        let (router, _engine) = emulator_with_engine();
        associate_admin(&router).await;

        let (status, put) = call(
            &router,
            "PutResourceSet",
            &json!({ "ResourceSet": case.resource_set, "TagList": case.tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected PutResourceSet to answer 200, got {status} with {put}",
            case.name
        );
        let stored = &put["ResourceSet"];
        let id = stored["Id"].as_str().unwrap_or_default().to_string();
        let arn = put["ResourceSetArn"].as_str().unwrap_or_default().to_string();

        // Id: exactly 22 characters from [a-z0-9A-Z], per the model's
        // `com.amazonaws.fms#Base62Id` pattern `^[a-z0-9A-Z]{22}$`.
        assert_eq!(
            id.chars().count(),
            22,
            "case {:?}: expected a 22-character Id, got {id:?}",
            case.name
        );
        assert!(
            id.chars().all(char::is_alphanumeric),
            "case {:?}: Id must match ^[a-z0-9A-Z]{{22}}$, got {id:?}",
            case.name
        );
        assert!(
            !stored["UpdateToken"].as_str().unwrap_or_default().is_empty(),
            "case {:?}: expected a minted UpdateToken, got {stored}",
            case.name
        );
        assert_eq!(
            arn,
            format!("arn:aws:fms:{REGION}:{ACCOUNT}:resource-set/{id}"),
            "case {:?}: unexpected ResourceSetArn, got {arn:?}",
            case.name
        );
        let reported_status = stored["ResourceSetStatus"].as_str().unwrap_or_default();
        assert!(
            statuses.contains(&reported_status),
            "case {:?}: {reported_status:?} is not in the model's ResourceSetStatus catalog \
             {statuses:?}",
            case.name
        );
        assert!(
            stored["LastUpdateTime"].is_number(),
            "case {:?}: expected a LastUpdateTime, got {stored}",
            case.name
        );

        // Everything the caller sent survives the round trip.
        for (member, sent) in case
            .resource_set
            .as_object()
            .expect("the case resource set is an object")
        {
            assert_eq!(
                stored.get(member),
                Some(sent),
                "case {:?}: member {member} was not stored as sent, got {:?}",
                case.name,
                stored.get(member)
            );
        }

        let (status, got) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetResourceSet to answer 200, got {status} with {got}",
            case.name
        );
        assert_eq!(
            got["ResourceSet"], *stored,
            "case {:?}: GetResourceSet must report what PutResourceSet stored, got {}",
            case.name, got["ResourceSet"]
        );
        assert_eq!(
            got["ResourceSetArn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: GetResourceSet must report the minted ARN, got {}",
            case.name,
            got["ResourceSetArn"]
        );
    }
}

/// The `UpdateToken` rules the member's own documentation states: not allowed on
/// a create, optional on an update (a token that is sent must match), rotated by
/// every successful write; a resource set cannot be renamed; and an `Id` that
/// names nothing is refused.
#[tokio::test]
async fn a_resource_set_update_follows_the_update_token_rules() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    // A create may not carry an UpdateToken.
    let (status, body) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": "with-token",
                "ResourceTypeList": ["AWS::EC2::Instance"],
                "UpdateToken": "not-allowed-on-create",
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "a create carrying an UpdateToken must be refused, got {status} with {body}"
    );

    let (_, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": { "Name": "updatable", "ResourceTypeList": ["AWS::EC2::Instance"] },
        }),
    )
    .await;
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .expect("a minted Id")
        .to_string();
    let first_token = created["ResourceSet"]["UpdateToken"]
        .as_str()
        .expect("a minted UpdateToken")
        .to_string();

    // A conditional update carrying the current token succeeds and rotates it.
    let (status, updated) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Id": id,
                "Name": "updatable",
                "Description": "now described",
                "ResourceTypeList": ["AWS::EC2::Instance", "AWS::EC2::VPC"],
                "UpdateToken": first_token,
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update with the current token must succeed, got {status} with {updated}"
    );
    let second_token = updated["ResourceSet"]["UpdateToken"]
        .as_str()
        .expect("a rotated UpdateToken")
        .to_string();
    assert_ne!(
        second_token, first_token,
        "a successful update must rotate the UpdateToken, got {second_token:?}"
    );
    assert_eq!(
        updated["ResourceSet"]["Description"].as_str(),
        Some("now described"),
        "the update must replace the document, got {}",
        updated["ResourceSet"]
    );

    // The token that just worked is now stale.
    let (status, body) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Id": id,
                "Name": "updatable",
                "ResourceTypeList": ["AWS::EC2::Instance"],
                "UpdateToken": first_token,
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "a stale UpdateToken must be refused, got {status} with {body}"
    );

    // An unconditional update (no token) is allowed, and rotates the token too.
    let (status, unconditional) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Id": id,
                "Name": "updatable",
                "ResourceTypeList": ["AWS::EC2::Instance"],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update without a token must be allowed, got {status} with {unconditional}"
    );
    assert_ne!(
        unconditional["ResourceSet"]["UpdateToken"].as_str(),
        Some(second_token.as_str()),
        "an unconditional update must rotate the UpdateToken too, got {}",
        unconditional["ResourceSet"]
    );

    // The name cannot change after creation.
    let (status, body) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Id": id,
                "Name": "renamed",
                "ResourceTypeList": ["AWS::EC2::Instance"],
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "renaming a resource set must be refused, got {status} with {body}"
    );

    // An Id that names nothing is refused rather than created under that id.
    let (status, body) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Id": UNKNOWN_RESOURCE_SET_ID,
                "Name": "ghost",
                "ResourceTypeList": ["AWS::EC2::Instance"],
            },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "an unknown Id must be refused, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "GetResourceSet",
        &json!({ "Identifier": UNKNOWN_RESOURCE_SET_ID }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "the refused update must not have created a resource set, got {status} with {body}"
    );
}

/// One row of the item-classification table: a URI and the `FailedItemReason` it
/// must be reported with, or `None` when it must be associated.
struct ItemCase {
    uri: String,
    reason: Option<&'static str>,
}

/// `BatchAssociateResource` classifies each item INDEPENDENTLY against the field
/// of its ARN that disagrees, associates the rest of the batch, and is
/// idempotent; an identifier that names no resource set is
/// `ResourceNotFoundException`.
#[tokio::test]
async fn batch_associate_resource_classifies_each_item_and_associates_the_rest() {
    let reasons = service_com_amazonaws_fms_awsfms_20180101::ops::types::FAILED_ITEM_REASON_VALUES;
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // An account this organization holds beside the administrator's own: a policy
    // scope map names it, which is exactly what `ListMemberAccounts` reports as a
    // member account — so a resource of that account is associable.
    let member = "222233334444";
    let (status, body) = call(
        &router,
        "PutPolicy",
        &json!({
            "Policy": {
                "PolicyName": "scopes-a-member-account",
                "SecurityServicePolicyData": {
                    "Type": "WAFV2",
                    "ManagedServiceData": "{\"type\":\"WAFV2\"}"
                },
                // A resource type the model lists for a WAFV2 policy; what this
                // policy is here for is the scope map below.
                "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
                "ExcludeResourceTags": false,
                "RemediationEnabled": false,
                "IncludeMap": { "ACCOUNT": [member] },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutPolicy must store the scope map that makes {member} a member account, got {status} \
         with {body}"
    );

    let (_, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": "associable",
                "ResourceTypeList": ["AWS::NetworkFirewall::Firewall", "AWS::EC2::Instance"],
            },
        }),
    )
    .await;
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .expect("a minted Id")
        .to_string();
    let arn = created["ResourceSetArn"]
        .as_str()
        .expect("a minted ARN")
        .to_string();

    // Rows differ in WHICH field of the ARN disagrees, one per reason the
    // model's `FailedItemReason` declares for a field this emulator can judge.
    let cases = vec![
        ItemCase {
            uri: format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/edge"),
            reason: None,
        },
        ItemCase {
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef0"),
            reason: None,
        },
        ItemCase {
            // Another account of the same organization, known through the policy
            // scope map above — the membership `ListMemberAccounts` reports.
            uri: format!("arn:aws:ec2:{REGION}:{member}:instance/i-0123456789abcdef1"),
            reason: None,
        },
        ItemCase {
            uri: "this-is-not-an-arn-at-all".to_string(),
            reason: Some("NOT_VALID_ARN"),
        },
        ItemCase {
            uri: format!("arn:aws-cn:network-firewall:{REGION}:{ACCOUNT}:firewall/edge"),
            reason: Some("NOT_VALID_PARTITION"),
        },
        ItemCase {
            uri: format!("arn:aws:network-firewall:{OTHER_REGION}:{ACCOUNT}:firewall/edge"),
            reason: Some("NOT_VALID_REGION"),
        },
        ItemCase {
            uri: format!("arn:aws:network-firewall:{REGION}:999999999999:firewall/edge"),
            reason: Some("NOT_VALID_ACCOUNT_ID"),
        },
        ItemCase {
            uri: format!(
                "arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT}:loadbalancer/app/edge/0123"
            ),
            reason: Some("NOT_VALID_RESOURCE_TYPE"),
        },
    ];
    for case in &cases {
        if let Some(reason) = case.reason {
            assert!(
                reasons.contains(&reason),
                "case {:?}: {reason:?} is not in the model's FailedItemReason catalog {reasons:?}",
                case.uri
            );
        }
    }

    let items: Vec<&str> = cases.iter().map(|case| case.uri.as_str()).collect();
    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": items }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchAssociateResource to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceSetIdentifier"].as_str(),
        Some(id.as_str()),
        "the response must name the resource set that was associated, got {body}"
    );
    let failed = body["FailedItems"]
        .as_array()
        .expect("FailedItems is a list")
        .clone();
    for case in &cases {
        let reported = failed
            .iter()
            .find(|item| item["URI"].as_str() == Some(case.uri.as_str()))
            .map(|item| item["Reason"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            reported.as_deref(),
            case.reason,
            "case {:?}: expected reason {:?}, got {reported:?} in {body}",
            case.uri,
            case.reason
        );
    }

    // The items that passed are the resource set's membership now.
    let expected: Vec<String> = cases
        .iter()
        .filter(|case| case.reason.is_none())
        .map(|case| case.uri.clone())
        .collect();
    with_store(&engine, |store| {
        let stored: Vec<String> = store.resource_sets[&id].resources.iter().cloned().collect();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            stored, wanted,
            "the associated items must be the resource set's membership, got {stored:?}"
        );
    });

    // Associating the same items again changes nothing: membership is a set.
    let (status, again) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": arn, "Items": expected }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the resource set can be named by its ARN too, got {status} with {again}"
    );
    assert_eq!(
        again["FailedItems"].as_array().map(Vec::len),
        Some(0),
        "re-associating the same items must fail nothing, got {again}"
    );
    with_store(&engine, |store| {
        assert_eq!(
            store.resource_sets[&id].resources.len(),
            expected.len(),
            "re-associating must not duplicate membership, got {:?}",
            store.resource_sets[&id].resources
        );
    });

    // An identifier that names no resource set is not found.
    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({
            "ResourceSetIdentifier": UNKNOWN_RESOURCE_SET_ID,
            "Items": [format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-1")],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "an unknown resource set must be ResourceNotFoundException, got {status} with {body}"
    );
}

/// Both sizes a caller controls are bounded: one call may carry only so many
/// items, and a resource set may hold only so many resources, so neither the
/// per-call work nor the stored membership grows without limit.
#[tokio::test]
async fn the_batch_and_the_resource_set_membership_are_bounded() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let (_, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": { "Name": "bounded", "ResourceTypeList": ["AWS::EC2::Instance"] },
        }),
    )
    .await;
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .expect("a minted Id")
        .to_string();

    /// One item ARN per index, all well formed and all associable.
    fn instance(index: usize) -> String {
        format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-{index:017x}")
    }

    // A call larger than the per-call bound is refused before anything is
    // classified, so it associates nothing.
    let oversized: Vec<String> = (0..1_000).map(instance).collect();
    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": oversized }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "an oversized batch must be refused, got {status} with {body}"
    );
    with_store(&engine, |store| {
        assert!(
            store.resource_sets[&id].resources.is_empty(),
            "a refused batch must associate nothing, got {:?}",
            store.resource_sets[&id].resources
        );
    });

    // Filling the set up to its bound, a bounded batch at a time, works; the
    // call that would cross the bound is refused with the modelled
    // LimitExceededException, and the membership stays at the bound.
    let mut associated = 0_usize;
    let mut limit_reached = None;
    for batch in 0..20 {
        let items: Vec<String> = (0..100).map(|index| instance(batch * 100 + index)).collect();
        let (status, body) = call(
            &router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": items }),
        )
        .await;
        if status == 200 {
            assert_eq!(
                body["FailedItems"].as_array().map(Vec::len),
                Some(0),
                "batch {batch}: every item is associable, got {body}"
            );
            associated += 100;
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "LimitExceededException"),
            "batch {batch}: crossing the membership bound must be LimitExceededException, got \
             {status} with {body}"
        );
        limit_reached = Some(batch);
        break;
    }
    assert!(
        limit_reached.is_some(),
        "the membership must be bounded, {associated} resources were associated without a refusal"
    );
    with_store(&engine, |store| {
        let held = store.resource_sets[&id].resources.len();
        assert_eq!(
            held, associated,
            "the refused batch must not have grown the membership, which holds {held}"
        );
        assert!(
            held <= 500,
            "the membership must stay within its bound, which holds {held}"
        );
    });

    // Re-associating resources the set already holds is still accepted at the
    // bound: it adds nothing, so it cannot cross it.
    let repeat: Vec<String> = (0..100).map(instance).collect();
    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": repeat }),
    )
    .await;
    assert_eq!(
        status, 200,
        "re-associating held resources adds nothing, so it must be accepted, got {status} with \
         {body}"
    );
}

/// `GetProtocolsList` reports a stored list and `DeleteProtocolsList` /
/// `DeleteAppsList` remove one from state, so a second call finds nothing;
/// `DefaultList` selects which namespace the id is resolved in.
#[tokio::test]
async fn the_lists_are_read_and_deleted_from_state() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // Rows differ in protocol count and in name form.
    let customer_lists = [
        (UNKNOWN_LIST_ID, "tcp-only", vec!["TCP"]),
        (
            OTHER_UNKNOWN_LIST_ID,
            "list.with-many_protocols",
            vec!["TCP", "UDP", "ICMP", "ESP"],
        ),
    ];
    with_store(&engine, |store| {
        for (list_id, list_name, protocols) in &customer_lists {
            seed_protocols_list(store, list_id, list_name, protocols, false);
        }
        seed_apps_list(store, REGION, UNKNOWN_LIST_ID, "an-apps-list", &[], false);
    });

    for (list_id, list_name, protocols) in &customer_lists {
        let (status, got) = call(&router, "GetProtocolsList", &json!({ "ListId": list_id })).await;
        assert_eq!(
            status, 200,
            "case {list_id:?}: expected GetProtocolsList to answer 200, got {status} with {got}"
        );
        assert_eq!(
            got["ProtocolsList"]["ListId"].as_str(),
            Some(*list_id),
            "case {list_id:?}: the list must report its own id, got {got}"
        );
        assert_eq!(
            got["ProtocolsList"]["ListName"].as_str(),
            Some(*list_name),
            "case {list_id:?}: the list must report its stored name, got {got}"
        );
        assert_eq!(
            got["ProtocolsList"]["ProtocolsList"],
            json!(protocols),
            "case {list_id:?}: the list must report its stored protocols, got {got}"
        );
        assert_eq!(
            got["ProtocolsListArn"].as_str(),
            Some(state::protocols_list_arn(REGION, list_id).as_str()),
            "case {list_id:?}: unexpected ProtocolsListArn, got {got}"
        );
        assert!(
            got["ProtocolsList"]["ListUpdateToken"]
                .as_str()
                .is_some_and(|token| !token.is_empty()),
            "case {list_id:?}: the list must report its update token, got {got}"
        );

        // A customer list's id does not name a Firewall Manager managed list.
        let (status, body) = call(
            &router,
            "GetProtocolsList",
            &json!({ "ListId": list_id, "DefaultList": true }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {list_id:?}: a customer list is not a default list, got {status} with {body}"
        );
    }

    // Deleting removes the list from state, so the read and a second delete both
    // find nothing.
    let (first, _, _) = &customer_lists[0];
    let (status, body) = call(&router, "DeleteProtocolsList", &json!({ "ListId": first })).await;
    assert_eq!(
        status, 200,
        "expected DeleteProtocolsList to answer 200, got {status} with {body}"
    );
    for operation in ["GetProtocolsList", "DeleteProtocolsList"] {
        let (status, body) = call(&router, operation, &json!({ "ListId": first })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation:?}: a deleted list must be gone, got {status} with {body}"
        );
    }
    // The OTHER list is untouched by that delete.
    let (status, _) = call(
        &router,
        "GetProtocolsList",
        &json!({ "ListId": customer_lists[1].0 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting one list must not remove another, got {status}"
    );

    // The applications list is a separate namespace with the same rules.
    let (status, body) = call(
        &router,
        "DeleteAppsList",
        &json!({ "ListId": UNKNOWN_LIST_ID }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteAppsList to answer 200, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "DeleteAppsList",
        &json!({ "ListId": UNKNOWN_LIST_ID }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "a deleted applications list must be gone, got {status} with {body}"
    );
    with_store(&engine, |store| {
        assert!(
            store.apps_lists.is_empty(),
            "the deleted applications list must be gone from state, got {:?}",
            store.apps_lists
        );
    });
}

/// `ListDiscoveredResources` reports the organisation's inventory filtered by the
/// member account and the resource type asked for, in bounded pages; with
/// nothing discovered it reports an empty list, never an invented resource.
#[tokio::test]
async fn list_discovered_resources_filters_and_pages_the_inventory() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // Nothing has been discovered: an empty page, and no token to follow.
    let (status, body) = call(
        &router,
        "ListDiscoveredResources",
        &json!({ "MemberAccountIds": [ACCOUNT], "ResourceType": "AWS::EC2::Instance" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected ListDiscoveredResources to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["Items"],
        json!([]),
        "an empty inventory must report no items, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "an empty page carries no NextToken, got {body}"
    );

    // Seeded inventory: two accounts, two resource types, so each filter has
    // something to exclude.
    let member = "222233334444";
    let seeded = [
        (ACCOUNT, "AWS::EC2::Instance", "i-000000000000000a"),
        (ACCOUNT, "AWS::EC2::Instance", "i-000000000000000b"),
        (ACCOUNT, "AWS::EC2::Instance", "i-000000000000000c"),
        (ACCOUNT, "AWS::EC2::VPC", "vpc-00000000000000aa"),
        (member, "AWS::EC2::Instance", "i-000000000000000d"),
    ];
    with_store(&engine, |store| {
        for (account, resource_type, name) in &seeded {
            let prefix = if *resource_type == "AWS::EC2::VPC" {
                "vpc"
            } else {
                "instance"
            };
            let uri = format!("arn:aws:ec2:{REGION}:{account}:{prefix}/{name}");
            store.discovered_resources.insert(
                uri.clone(),
                DiscoveredResource {
                    uri: Some(uri),
                    account_id: Some((*account).to_string()),
                    r#type: Some((*resource_type).to_string()),
                    name: Some((*name).to_string()),
                },
            );
        }
    });

    // Each (account, type) pair reports exactly the seeded resources that match.
    for (account, resource_type) in [
        (ACCOUNT, "AWS::EC2::Instance"),
        (ACCOUNT, "AWS::EC2::VPC"),
        (member, "AWS::EC2::Instance"),
        (member, "AWS::EC2::VPC"),
    ] {
        let expected = seeded
            .iter()
            .filter(|(seeded_account, seeded_type, _)| {
                *seeded_account == account && *seeded_type == resource_type
            })
            .count();
        let (status, body) = call(
            &router,
            "ListDiscoveredResources",
            &json!({ "MemberAccountIds": [account], "ResourceType": resource_type }),
        )
        .await;
        let items = body["Items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            (status, items.len()),
            (200, expected),
            "case {account}/{resource_type}: expected {expected} items, got {status} with {body}"
        );
        for item in &items {
            assert_eq!(
                (item["AccountId"].as_str(), item["Type"].as_str()),
                (Some(account), Some(resource_type)),
                "case {account}/{resource_type}: an item outside the filter was reported: {item}"
            );
        }
    }

    // `MaxResults` bounds the page, and the token walks the rest exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..4 {
        let mut request = json!({
            "MemberAccountIds": [ACCOUNT],
            "ResourceType": "AWS::EC2::Instance",
            "MaxResults": 1,
        });
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(&router, "ListDiscoveredResources", &request).await;
        let items = body["Items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            (status, items.len()),
            (200, 1),
            "page {page}: expected a one-item page, got {status} with {body}"
        );
        seen.push(items[0]["URI"].as_str().unwrap_or_default().to_string());
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen.len(),
        3,
        "paging must report each of this account's instances once, got {seen:?}"
    );
    assert!(
        token.is_none(),
        "the last page must carry no NextToken, got {token:?}"
    );
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        seen.len(),
        "paging must not repeat a resource, got {seen:?}"
    );

    // A token this service did not mint, and more than one account, are refused.
    let (status, body) = call(
        &router,
        "ListDiscoveredResources",
        &json!({
            "MemberAccountIds": [ACCOUNT],
            "ResourceType": "AWS::EC2::Instance",
            "NextToken": "not-a-token-we-minted",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "a foreign NextToken must be refused, got {status} with {body}"
    );
    for accounts in [json!([]), json!([ACCOUNT, member])] {
        let (status, body) = call(
            &router,
            "ListDiscoveredResources",
            &json!({ "MemberAccountIds": accounts, "ResourceType": "AWS::EC2::Instance" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {accounts}: only one account is supported per request, got {status} with {body}"
        );
    }
}

/// With no association to a vendor the status is `NOT_EXIST` /`NO_SUBSCRIPTION`;
/// `DisassociateThirdPartyFirewall` starts the teardown, which is observable as
/// `OFFBOARDING` straight after the call and settles to `OFFBOARD_COMPLETE` once
/// the clock makes it due.
#[tokio::test]
async fn disassociating_a_third_party_firewall_offboards_and_settles() {
    let vendors = service_com_amazonaws_fms_awsfms_20180101::ops::types::THIRD_PARTY_FIREWALL_VALUES;
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // Every vendor the model declares, not only the one the issue cited.
    for vendor in vendors {
        let (status, body) = call(
            &router,
            "GetThirdPartyFirewallAssociationStatus",
            &json!({ "ThirdPartyFirewall": vendor }),
        )
        .await;
        assert_eq!(
            (
                status,
                body["ThirdPartyFirewallStatus"].as_str(),
                body["MarketplaceOnboardingStatus"].as_str(),
            ),
            (200, Some("NOT_EXIST"), Some("NO_SUBSCRIPTION")),
            "case {vendor}: an unassociated vendor must report NOT_EXIST, got {status} with {body}"
        );
        let (status, body) = call(
            &router,
            "DisassociateThirdPartyFirewall",
            &json!({ "ThirdPartyFirewall": vendor }),
        )
        .await;
        assert_eq!(
            (status, body["ThirdPartyFirewallStatus"].as_str()),
            (200, Some("NOT_EXIST")),
            "case {vendor}: there is nothing to disassociate, got {status} with {body}"
        );
    }

    // An onboarded association for every vendor, as `AssociateThirdPartyFirewall`
    // will write it.
    with_store(&engine, |store| {
        for vendor in vendors {
            store.third_party_firewalls.insert(
                (*vendor).to_string(),
                state::ThirdPartyFirewallRecord {
                    status: "ONBOARD_COMPLETE".to_string(),
                    settles_to: None,
                    settles_at: None,
                    marketplace_onboarding_status: "COMPLETE".to_string(),
                },
            );
        }
    });

    for vendor in vendors {
        let (status, body) = call(
            &router,
            "GetThirdPartyFirewallAssociationStatus",
            &json!({ "ThirdPartyFirewall": vendor }),
        )
        .await;
        assert_eq!(
            (
                status,
                body["ThirdPartyFirewallStatus"].as_str(),
                body["MarketplaceOnboardingStatus"].as_str(),
            ),
            (200, Some("ONBOARD_COMPLETE"), Some("COMPLETE")),
            "case {vendor}: the stored association must be reported, got {status} with {body}"
        );
    }

    // Tearing one down leaves the other alone, and is observable as OFFBOARDING
    // before the clock settles it.
    let (torn_down, untouched) = (vendors[0], vendors[1]);
    let (status, body) = call(
        &router,
        "DisassociateThirdPartyFirewall",
        &json!({ "ThirdPartyFirewall": torn_down }),
    )
    .await;
    assert_eq!(
        (status, body["ThirdPartyFirewallStatus"].as_str()),
        (200, Some("OFFBOARDING")),
        "case {torn_down}: the teardown must start in OFFBOARDING, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "GetThirdPartyFirewallAssociationStatus",
        &json!({ "ThirdPartyFirewall": torn_down }),
    )
    .await;
    assert_eq!(
        (status, body["ThirdPartyFirewallStatus"].as_str()),
        (200, Some("OFFBOARDING")),
        "case {torn_down}: a caller reading straight after must still see OFFBOARDING, got \
         {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "GetThirdPartyFirewallAssociationStatus",
        &json!({ "ThirdPartyFirewall": untouched }),
    )
    .await;
    assert_eq!(
        (status, body["ThirdPartyFirewallStatus"].as_str()),
        (200, Some("ONBOARD_COMPLETE")),
        "case {untouched}: another vendor's association must be untouched, got {status} with \
         {body}"
    );

    // Past the stored due instant the terminal status is what a reader sees.
    tokio::time::sleep(state::ASSOCIATION_SETTLE + Duration::from_millis(200)).await;
    let (status, body) = call(
        &router,
        "GetThirdPartyFirewallAssociationStatus",
        &json!({ "ThirdPartyFirewall": torn_down }),
    )
    .await;
    assert_eq!(
        (status, body["ThirdPartyFirewallStatus"].as_str()),
        (200, Some("OFFBOARD_COMPLETE")),
        "case {torn_down}: the clock must settle the teardown, got {status} with {body}"
    );
    // A second teardown has nothing left to start.
    let (status, body) = call(
        &router,
        "DisassociateThirdPartyFirewall",
        &json!({ "ThirdPartyFirewall": torn_down }),
    )
    .await;
    assert_eq!(
        (status, body["ThirdPartyFirewallStatus"].as_str()),
        (200, Some("OFFBOARD_COMPLETE")),
        "case {torn_down}: a second teardown must be idempotent, got {status} with {body}"
    );
}

/// Reads do not change what a later caller sees: every reporting operation this
/// change implements answers identically when it is repeated.
#[tokio::test]
async fn reads_are_stable_across_repeated_calls() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    call(
        &router,
        "PutNotificationChannel",
        &json!({
            "SnsTopicArn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:fms"),
            "SnsRoleName": format!("arn:aws:iam::{ACCOUNT}:role/fms"),
        }),
    )
    .await;
    let (_, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": { "Name": "stable", "ResourceTypeList": ["AWS::EC2::Instance"] },
        }),
    )
    .await;
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .expect("a minted Id")
        .to_string();
    with_store(&engine, |store| {
        seed_protocols_list(store, UNKNOWN_LIST_ID, "stable", &["TCP"], false);
    });

    let reads = [
        ("GetNotificationChannel", json!({})),
        ("GetResourceSet", json!({ "Identifier": id })),
        ("GetProtocolsList", json!({ "ListId": UNKNOWN_LIST_ID })),
        ("ListProtocolsLists", json!({ "MaxResults": 100 })),
        (
            "ListDiscoveredResources",
            json!({ "MemberAccountIds": [ACCOUNT], "ResourceType": "AWS::EC2::Instance" }),
        ),
        (
            "GetThirdPartyFirewallAssociationStatus",
            json!({ "ThirdPartyFirewall": "PALO_ALTO_NETWORKS_CLOUD_NGFW" }),
        ),
    ];
    for (operation, body) in &reads {
        let first = call(&router, operation, body).await;
        let second = call(&router, operation, body).await;
        assert_eq!(
            first, second,
            "case {operation:?}: a read must not change what the next caller sees, got {first:?} \
             then {second:?}"
        );
    }
}
