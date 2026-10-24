// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `Message` member of this service's client-error
//! bodies: that it is THERE under the name the service's error shapes declare,
//! and that its content follows the rule the error it belongs to follows.
//!
//! Two rules, each asserted over a table rather than over one request:
//!
//! 1. Every client error this service answers carries its message under the
//!    member name the model declares for it — `Message` (every error shape in
//!    `com.amazonaws.fms` declares exactly one member, `Message`, targeting
//!    `ErrorMessage`). On awsJson1_1 the MEMBER NAME is the JSON key, so a body
//!    that spells it any other way has, to a client, no message at all.
//! 2. `ResourceNotFoundException`'s message is a service CONSTANT: it is the
//!    same string whatever operation answered it and whatever was looked up, and
//!    it never restates the identifier, ARN or name from the request. The
//!    administrator-authorization refusal is the opposite kind of message — it is
//!    composed from the request, naming the calling account and the Region the
//!    request was signed for — so it moves with the Region while the not-found
//!    text does not move at all.
//!
//! The tables below are the classes those rules range over: the not-found table
//! spans every operation that can answer not-found today and three kinds of
//! lookup key (a policy UUID, a list/resource-set id, a resource ARN) plus the
//! two "nothing is configured at all" lookups, with two distinct keys per key
//! kind so an invariant text can be told from an interpolated one; the refusal
//! table spans several gated operations in several Regions.
//!
//! The wire harness is `policy_lifecycle`'s, so these assertions are about the
//! same deserialize → handle → serialize walk a client drives.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_lifecycle::{call, call_in, emulator, error_code, ACCOUNT, REGION};

/// The one `Message` Firewall Manager answers every `ResourceNotFoundException`
/// with.
///
/// Source: the recordings of the live service in trace set
/// `simparity-wire-aws-v1`, where this identical literal is the message of the
/// not-found error on five different operations (`ListAdminsManagingAccount`,
/// `GetPolicy`, `DeletePolicy`, `ListTagsForResource`, `TagResource`), across
/// three fixtures and across two kinds of lookup key — never the key itself. The
/// model declares the `Message` member but no literal for it.
const NOT_FOUND_MESSAGE: &str = "The referenced item does not exist.";

/// A `PolicyId` (the model's `com.amazonaws.fms#PolicyId`: 36 characters
/// matching `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "109c24c6-0d5e-407e-98b5-8619b6cf4907";

/// A second well-formed `PolicyId` that names no policy, so a message that
/// interpolated the id could not pass the same assertion as the first.
const OTHER_UNKNOWN_POLICY_ID: &str = "7b1f0e52-3c44-4a90-8d61-0ac5e9f2b8d3";

/// A `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no list.
const UNKNOWN_LIST_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A second well-formed `ListId` that names no list.
const OTHER_UNKNOWN_LIST_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A `Base62Id` (`com.amazonaws.fms#Base62Id`: 22 characters matching
/// `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "0123456789AbCdEfGhIjKl";

/// A second well-formed `Base62Id` that names no resource set.
const OTHER_UNKNOWN_RESOURCE_SET_ID: &str = "ZyXwVuTsRqPoNmLkJiHgFe";

/// Regions the refusal is asserted in: the suite's own Region plus two others,
/// so a message composed from the request can be told from a constant.
const REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-southeast-4"];

/// The message member of an awsJson1_1 error body, read under the EXACT name the
/// service's error shapes declare. Deliberately NOT tolerant of other casings:
/// the key is what a client reads the exception message from.
fn message_member(body: &Value) -> Option<&str> {
    body.get("Message").and_then(Value::as_str)
}

/// One row of a table: a name for the assertion messages, the operation, the
/// request, and the identifier the request looked up (if any), which must NOT
/// appear in the answer.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    lookup: Option<String>,
}

/// An `fms` ARN that names no stored resource.
fn unknown_arn(resource: &str) -> String {
    format!("arn:aws:fms:{REGION}:{ACCOUNT}:{resource}")
}

/// A policy document `PutPolicy` accepts, carrying the model's required members
/// (`PolicyName`, `SecurityServicePolicyData`, `ResourceType`,
/// `ExcludeResourceTags`, `RemediationEnabled`) plus the id and token that make
/// the call an UPDATE of an existing policy version.
fn policy_update(policy_id: &str) -> Value {
    json!({
        "PolicyId": policy_id,
        "PolicyUpdateToken": "an-update-token",
        "PolicyName": "not.a.stored.policy",
        "SecurityServicePolicyData": {
            "Type": "WAFV2",
            "ManagedServiceData": "{\"type\":\"WAFV2\"}"
        },
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    })
}

/// Not-found lookups that need NO administrator association: the two that are
/// not-found precisely BECAUSE nothing is associated, the policy lookups and the
/// ARN-keyed tagging lookups (none of which is organisation-scoped).
fn not_found_without_administrator() -> Vec<Row> {
    vec![
        Row {
            case: "GetAdminAccount with no association at all",
            operation: "GetAdminAccount",
            body: json!({}),
            lookup: None,
        },
        Row {
            case: "ListAdminsManagingAccount when no administrator manages the caller",
            operation: "ListAdminsManagingAccount",
            body: json!({}),
            lookup: None,
        },
        Row {
            case: "GetPolicy with an unknown policy id",
            operation: "GetPolicy",
            body: json!({ "PolicyId": UNKNOWN_POLICY_ID }),
            lookup: Some(UNKNOWN_POLICY_ID.to_string()),
        },
        Row {
            case: "GetPolicy with a second, different unknown policy id",
            operation: "GetPolicy",
            body: json!({ "PolicyId": OTHER_UNKNOWN_POLICY_ID }),
            lookup: Some(OTHER_UNKNOWN_POLICY_ID.to_string()),
        },
        Row {
            case: "DeletePolicy with an unknown policy id",
            operation: "DeletePolicy",
            body: json!({ "PolicyId": UNKNOWN_POLICY_ID }),
            lookup: Some(UNKNOWN_POLICY_ID.to_string()),
        },
        Row {
            case: "PutPolicy updating an unknown policy id",
            operation: "PutPolicy",
            body: json!({ "Policy": policy_update(OTHER_UNKNOWN_POLICY_ID) }),
            lookup: Some(OTHER_UNKNOWN_POLICY_ID.to_string()),
        },
        Row {
            case: "ListTagsForResource with a policy ARN naming no policy",
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": unknown_arn("policy/5cd98ad7-93fc-4c5c-9faf-8cd2b03b2bb8") }),
            lookup: Some(unknown_arn("policy/5cd98ad7-93fc-4c5c-9faf-8cd2b03b2bb8")),
        },
        Row {
            case: "ListTagsForResource with a resource-set ARN naming nothing",
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": unknown_arn("resource-set/0123456789AbCdEfGhIjKl") }),
            lookup: Some(unknown_arn("resource-set/0123456789AbCdEfGhIjKl")),
        },
        Row {
            case: "TagResource with a policy ARN naming no policy",
            operation: "TagResource",
            body: json!({
                "ResourceArn": unknown_arn("policy/5cd98ad7-93fc-4c5c-9faf-8cd2b03b2bb8"),
                "TagList": [{ "Key": "stage", "Value": "test" }],
            }),
            lookup: Some(unknown_arn("policy/5cd98ad7-93fc-4c5c-9faf-8cd2b03b2bb8")),
        },
        Row {
            case: "TagResource with an applications-list ARN naming nothing",
            operation: "TagResource",
            body: json!({
                "ResourceArn": unknown_arn("applications-list/11111111-2222-3333-4444-555555555555"),
                "TagList": [{ "Key": "owner", "Value": "team" }],
            }),
            lookup: Some(unknown_arn(
                "applications-list/11111111-2222-3333-4444-555555555555",
            )),
        },
    ]
}

/// Not-found lookups the administrator gate guards: these answer the refusal
/// until an administrator is associated, and the modelled not-found after.
fn not_found_with_administrator() -> Vec<Row> {
    vec![
        Row {
            case: "DeleteAppsList with an unknown list id",
            operation: "DeleteAppsList",
            body: json!({ "ListId": UNKNOWN_LIST_ID }),
            lookup: Some(UNKNOWN_LIST_ID.to_string()),
        },
        Row {
            case: "DeleteProtocolsList with a second unknown list id",
            operation: "DeleteProtocolsList",
            body: json!({ "ListId": OTHER_UNKNOWN_LIST_ID }),
            lookup: Some(OTHER_UNKNOWN_LIST_ID.to_string()),
        },
        Row {
            case: "GetProtocolsList with an unknown list id",
            operation: "GetProtocolsList",
            body: json!({ "ListId": UNKNOWN_LIST_ID, "DefaultList": true }),
            lookup: Some(UNKNOWN_LIST_ID.to_string()),
        },
        Row {
            case: "GetResourceSet with an unknown identifier",
            operation: "GetResourceSet",
            body: json!({ "Identifier": UNKNOWN_RESOURCE_SET_ID }),
            lookup: Some(UNKNOWN_RESOURCE_SET_ID.to_string()),
        },
        Row {
            case: "DeleteResourceSet with an unknown identifier",
            operation: "DeleteResourceSet",
            body: json!({ "Identifier": OTHER_UNKNOWN_RESOURCE_SET_ID }),
            lookup: Some(OTHER_UNKNOWN_RESOURCE_SET_ID.to_string()),
        },
        Row {
            case: "BatchAssociateResource with a second unknown resource-set identifier",
            operation: "BatchAssociateResource",
            body: json!({
                "ResourceSetIdentifier": OTHER_UNKNOWN_RESOURCE_SET_ID,
                "Items": [format!("arn:aws:ec2:{REGION}:{ACCOUNT}:vpc/vpc-0123456789abcdef0")],
            }),
            lookup: Some(OTHER_UNKNOWN_RESOURCE_SET_ID.to_string()),
        },
        Row {
            case: "GetNotificationChannel with no channel configured",
            operation: "GetNotificationChannel",
            body: json!({}),
            lookup: None,
        },
    ]
}

/// Make this account the Firewall Manager default administrator, which opens the
/// gate in front of the organisation-scoped rows.
async fn associate_admin(router: &MultiServiceRouter) {
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

/// Drive one row and assert the not-found rule on it, returning the `Message` it
/// answered so the caller can assert the whole table agreed on ONE string.
async fn assert_not_found(router: &MultiServiceRouter, row: &Row) -> String {
    let (status, body) = call(router, row.operation, &row.body).await;
    assert_eq!(
        status, 400,
        "case {:?}: expected HTTP 400, got {status} with {body}",
        row.case
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {:?}: expected the modelled not-found error, got {body}",
        row.case
    );
    let Some(message) = message_member(&body) else {
        panic!(
            "case {:?}: the error body must carry its message under the member name the model \
             declares (`Message`), got {body}",
            row.case
        );
    };
    assert_eq!(
        message, NOT_FOUND_MESSAGE,
        "case {:?}: the not-found message is a service constant, got {message:?}",
        row.case
    );
    if let Some(lookup) = &row.lookup {
        assert!(
            !message.contains(lookup.as_str()),
            "case {:?}: the not-found message must not restate the looked-up key {lookup:?}, \
             got {message:?}",
            row.case
        );
    }
    assert!(
        body.get("message").is_none(),
        "case {:?}: the body must carry exactly one message member, named `Message`, got {body}",
        row.case
    );
    message.to_string()
}

/// `ResourceNotFoundException` answers ONE constant `Message` on every operation
/// and for every kind of lookup key, and never restates the key.
#[tokio::test]
async fn resource_not_found_answers_one_constant_message_everywhere() {
    let mut messages: BTreeSet<String> = BTreeSet::new();

    let router = emulator();
    for row in not_found_without_administrator() {
        messages.insert(assert_not_found(&router, &row).await);
    }

    let gated = emulator();
    associate_admin(&gated).await;
    for row in not_found_with_administrator() {
        messages.insert(assert_not_found(&gated, &row).await);
    }

    assert_eq!(
        messages.len(),
        1,
        "the not-found message must not vary with the operation or the key, got {messages:?}"
    );
}

/// The not-found message does not move with the Region either: the SAME lookup
/// signed for a different Region answers the same constant, which is what tells
/// this error apart from the request-composed authorization refusal below.
#[tokio::test]
async fn resource_not_found_message_does_not_vary_with_the_region() {
    let router = emulator();
    for region in REGIONS {
        let (status, body) = call_in(
            &router,
            "GetPolicy",
            &json!({ "PolicyId": UNKNOWN_POLICY_ID }),
            region,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "region {region}: expected the modelled not-found error, got {status} with {body}"
        );
        assert_eq!(
            message_member(&body),
            Some(NOT_FOUND_MESSAGE),
            "region {region}: expected the constant not-found message, got {body}"
        );
    }
}

/// With no administrator associated, every gated operation's HTTP 400
/// `AccessDeniedException` carries a `Message` — under that member name — built
/// from the request: it names the calling account and the Region the request was
/// signed for, so it differs between Regions and agrees between operations in one
/// Region.
#[tokio::test]
async fn the_authorization_refusal_carries_a_message_built_from_the_request() {
    let gated_operations = [
        ("BatchAssociateResource", json!({
            "ResourceSetIdentifier": UNKNOWN_RESOURCE_SET_ID,
            "Items": [format!("arn:aws:ec2:{REGION}:{ACCOUNT}:vpc/vpc-0123456789abcdef0")],
        })),
        ("DeleteAppsList", json!({ "ListId": UNKNOWN_LIST_ID })),
        ("GetProtocolsList", json!({ "ListId": OTHER_UNKNOWN_LIST_ID, "DefaultList": false })),
        ("GetNotificationChannel", json!({})),
        ("GetResourceSet", json!({ "Identifier": UNKNOWN_RESOURCE_SET_ID })),
    ];

    let mut per_region: Vec<String> = Vec::new();
    for region in REGIONS {
        let router = emulator();
        let mut in_this_region: BTreeSet<String> = BTreeSet::new();
        for (operation, body) in &gated_operations {
            let (status, answer) = call_in(&router, operation, body, region).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "AccessDeniedException"),
                "case {operation} in {region}: expected the authorization refusal, got {status} \
                 with {answer}"
            );
            let Some(message) = message_member(&answer) else {
                panic!(
                    "case {operation} in {region}: the refusal must carry its message under the \
                     member name this service's error shapes declare (`Message`), got {answer}"
                );
            };
            assert!(
                message.contains(ACCOUNT) && message.contains(region),
                "case {operation} in {region}: the refusal's message is composed from the \
                 request, so it must name the calling account and the Region the administrator \
                 was looked for in, got {message:?}"
            );
            assert!(
                answer.get("message").is_none(),
                "case {operation} in {region}: the body must carry exactly one message member, \
                 named `Message`, got {answer}"
            );
            in_this_region.insert(message.to_string());
        }
        assert_eq!(
            in_this_region.len(),
            1,
            "region {region}: the refusal says the same thing whichever gated operation was \
             called, got {in_this_region:?}"
        );
        per_region.push(
            in_this_region
                .into_iter()
                .next()
                .expect("one message per Region"),
        );
    }

    let distinct: BTreeSet<&String> = per_region.iter().collect();
    assert_eq!(
        distinct.len(),
        REGIONS.len(),
        "the refusal is composed from the request, so each Region gets its own message, got \
         {per_region:?}"
    );
}
