// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE of a modelled identity-store exception — the HTTP status beside the error
//! code, and the members every exception body carries — driven end to end over HTTP
//! through [`crate::harness`].
//!
//! Two rules are under test, each asserted over a TABLE whose rows differ along what the
//! rule generalises over (the operation, the resource class, how the resource was named,
//! the error class):
//!
//! 1. a resource this service does not hold is reported as a CLIENT fault at the status
//!    this service answers its not-found class with ([`NOT_FOUND`]), with the not-found
//!    error code naming the class in `ResourceType` — whichever operation asked, whichever
//!    resource class is absent, and whether the resource was named by id, by an alternate
//!    identifier or through `MemberId`. Every OTHER error class keeps the status its own
//!    error shape models, which the last suite pins.
//! 2. every modelled exception this service returns carries a `RequestId` in the modelled
//!    form — five hexadecimal groups of 8-4-4-4-12 characters
//!    (`smithy_query(query="shape",shape="RequestId")`) — minted per call, so two
//!    identical failing calls report two different ids and no id of the request appears in
//!    it.
//!
//! Both rules are about the envelope and not about one operation, so the rows span the
//! operations that raise each class rather than the one an issue happened to cite.

use serde_json::{json, Value};

use crate::harness::{call, emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{create_group, create_user, user_name};

/// A `ResourceId` the modelled pattern admits (`smithy_query(query="shape",
/// shape="ResourceId")`) that no create in these suites ever minted.
const UNMINTED_ID: &str = "7f3a1b2c-4d5e-6f70-8192-a3b4c5d6e7f8";

/// A second one, so a case can name two resources neither of which exists.
const OTHER_UNMINTED_ID: &str = "0badc0de-1234-4321-8765-ffeeddccbbaa";

/// The group sizes of the modelled `RequestId` pattern
/// (`^[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`, from
/// `smithy_query(query="shape",shape="RequestId")`), which also fix its length at the
/// modelled maximum of 36 characters.
const REQUEST_ID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `value` is a `RequestId` the model admits.
fn is_modelled_request_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == REQUEST_ID_GROUPS.len()
        && groups
            .iter()
            .zip(REQUEST_ID_GROUPS)
            .all(|(group, size)| {
                group.len() == size && group.chars().all(|c| c.is_ascii_hexdigit())
            })
        && value.len() == 36
}

/// The `RequestId` of an exception body, asserting it is there and in the modelled form.
fn request_id(body: &Value, case: &str) -> String {
    let value = body["RequestId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_request_id(&value),
        "case {case:?}: expected a RequestId of 8-4-4-4-12 hexadecimal groups, \
         got {value:?} in {body}"
    );
    value
}

/// Rule 1: whatever the operation and whatever class of resource is missing, the answer is
/// the not-found error code at this service's client-fault status, naming the class.
#[tokio::test]
async fn a_resource_this_service_does_not_hold_is_a_client_fault_naming_its_class() {
    let router = emulator();
    let group = create_group(&router, STORE, "the group that exists", "fixtures").await;
    let user = create_user(
        &router,
        STORE,
        &user_name("the user that exists", 0),
        "fixtures",
    )
    .await;

    // (case, operation, request, the `ResourceType` the answer names)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a group id nothing minted, read by id",
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": UNMINTED_ID }),
            "GROUP",
        ),
        (
            "a user id nothing minted, read by id",
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": UNMINTED_ID }),
            "USER",
        ),
        (
            "a membership id nothing minted, read by id",
            "DescribeGroupMembership",
            json!({ "IdentityStoreId": STORE, "MembershipId": UNMINTED_ID }),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a group id nothing minted, on a listing",
            "ListGroupMemberships",
            json!({ "IdentityStoreId": STORE, "GroupId": UNMINTED_ID }),
            "GROUP",
        ),
        (
            "a group id nothing minted, on an update",
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": UNMINTED_ID,
                "Operations": [
                    { "AttributePath": "description", "AttributeValue": "never applied" },
                ],
            }),
            "GROUP",
        ),
        (
            "a membership id nothing minted, on a delete",
            "DeleteGroupMembership",
            json!({ "IdentityStoreId": STORE, "MembershipId": UNMINTED_ID }),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a member id no user carries, on a create",
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": UNMINTED_ID },
            }),
            "USER",
        ),
        (
            "a member id no user carries, on a membership check",
            "IsMemberInGroups",
            json!({
                "IdentityStoreId": STORE,
                "MemberId": { "UserId": UNMINTED_ID },
                "GroupIds": [group.clone()],
            }),
            "USER",
        ),
        (
            "a member id no user carries, on a listing",
            "ListGroupMembershipsForMember",
            json!({ "IdentityStoreId": STORE, "MemberId": { "UserId": UNMINTED_ID } }),
            "USER",
        ),
        (
            "a (group, member) pair no membership joins",
            "GetGroupMembershipId",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a display name no group carries, resolved by alternate identifier",
            "GetGroupId",
            json!({
                "IdentityStoreId": STORE,
                "AlternateIdentifier": {
                    "UniqueAttribute": {
                        "AttributePath": "displayName",
                        "AttributeValue": "no group carries this",
                    },
                },
            }),
            "GROUP",
        ),
        (
            "a user name no user carries, resolved by alternate identifier",
            "GetUserId",
            json!({
                "IdentityStoreId": STORE,
                "AlternateIdentifier": {
                    "UniqueAttribute": {
                        "AttributePath": "userName",
                        "AttributeValue": "no-user-carries-this",
                    },
                },
            }),
            "USER",
        ),
        (
            "a store this emulator does not serve, on a read",
            "DescribeGroup",
            json!({ "IdentityStoreId": MISSING_STORE, "GroupId": UNMINTED_ID }),
            "IDENTITY_STORE",
        ),
        (
            "a store this emulator does not serve, on a listing",
            "ListUsers",
            json!({ "IdentityStoreId": MISSING_STORE }),
            "IDENTITY_STORE",
        ),
        (
            "a store this emulator does not serve, on a create",
            "CreateGroup",
            json!({ "IdentityStoreId": MISSING_STORE, "DisplayName": "never stored" }),
            "IDENTITY_STORE",
        ),
    ];

    for (case, operation, request, resource_type) in rows {
        let (status, code, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.ends_with("ResourceNotFoundException")),
            (NOT_FOUND, true),
            "case {case:?}: expected {operation} to answer {NOT_FOUND} \
             ResourceNotFoundException, got {status} {code} with {body}"
        );
        assert_eq!(
            body["ResourceType"].as_str().unwrap_or_default(),
            resource_type,
            "case {case:?}: expected the missing resource to be reported as \
             {resource_type}, got {body}"
        );
        request_id(&body, case);
    }

    // Nothing above was created on the way: the store still holds exactly the two
    // resources the fixtures made.
    let groups = ok(
        &router,
        "ListGroups",
        json!({ "IdentityStoreId": STORE }),
        "after the refusals",
    )
    .await;
    assert_eq!(
        groups["Groups"].as_array().map(Vec::len),
        Some(1),
        "expected the refused calls to have stored nothing, got {groups}"
    );
}

/// Rule 2: every error class this service raises carries a `RequestId`, minted per call.
///
/// Rows differ along the error CLASS and the operation that raises it, which is what the
/// rule generalises over: the member is modelled on each of this service's exception
/// shapes, so it is populated on each.
#[tokio::test]
async fn every_modelled_exception_carries_a_request_id_minted_per_call() {
    let router = emulator();
    let taken = "the display name already taken";
    let group = create_group(&router, STORE, taken, "fixtures").await;
    let name = user_name("the user already created", 0);
    create_user(&router, STORE, &name, "fixtures").await;

    // (case, operation, request, the status and error code the class is answered with)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a resource the store does not hold",
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": UNMINTED_ID }),
            NOT_FOUND,
            "ResourceNotFoundException",
        ),
        (
            "a store this emulator does not serve",
            "ListGroups",
            json!({ "IdentityStoreId": MISSING_STORE }),
            NOT_FOUND,
            "ResourceNotFoundException",
        ),
        (
            "a display name another group already holds",
            "CreateGroup",
            json!({ "IdentityStoreId": STORE, "DisplayName": taken }),
            409,
            "ConflictException",
        ),
        (
            "a user name another user already holds",
            "CreateUser",
            json!({ "IdentityStoreId": STORE, "UserName": name.clone() }),
            409,
            "ConflictException",
        ),
        (
            "an expected revision the group is not at",
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "Revision": "999",
                "Operations": [
                    { "AttributePath": "description", "AttributeValue": "never applied" },
                ],
            }),
            409,
            "ConflictException",
        ),
        (
            "an attribute path a listing cannot filter on",
            "ListGroups",
            json!({
                "IdentityStoreId": STORE,
                "Filters": [{ "AttributePath": "description", "AttributeValue": "x" }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a cursor this service never issued",
            "ListUsers",
            json!({ "IdentityStoreId": STORE, "NextToken": "notacursor" }),
            400,
            "ValidationException",
        ),
        (
            "an attribute path an update does not own",
            "UpdateGroup",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "Operations": [
                    { "AttributePath": "groupId", "AttributeValue": OTHER_UNMINTED_ID },
                ],
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, operation, request, status, error) in rows {
        let first = refused(&router, operation, request.clone(), status, error, case).await;
        let second = refused(&router, operation, request, status, error, case).await;
        let first_id = request_id(&first, case);
        let second_id = request_id(&second, case);
        assert_ne!(
            first_id, second_id,
            "case {case:?}: expected a RequestId per call, got {first_id} twice"
        );
        // The id identifies the CALL: nothing the request carried can be read out of it.
        for sent in [UNMINTED_ID, OTHER_UNMINTED_ID, group.as_str(), name.as_str()] {
            assert_ne!(
                first_id, sent,
                "case {case:?}: expected the RequestId to be minted, not taken from the \
                 request, got {first_id}"
            );
        }
    }
}

/// The rule's BOUND: only the not-found class is answered at the client-fault status; every
/// other class is answered at the status its own error shape models
/// (`smithy_query(query="shape",shape="ConflictException,ServiceQuotaExceededException,
/// InternalServerException")`), and a request the boundary validator refuses stays the
/// `ValidationException` it has always been.
#[tokio::test]
async fn every_other_error_class_keeps_the_status_its_shape_models() {
    let router = emulator();
    let taken = "the display name a second create repeats";
    create_group(&router, STORE, taken, "fixtures").await;

    // A uniqueness conflict keeps the 409 `ConflictException` models.
    refused(
        &router,
        "CreateGroup",
        json!({ "IdentityStoreId": STORE, "DisplayName": taken }),
        409,
        "ConflictException",
        "a uniqueness claim",
    )
    .await;

    // A value the modelled constraints refuse is the boundary validator's 400, before any
    // handler runs — so it carries the code and message that path renders and no more.
    let (status, code, body) = call(
        &router,
        "DescribeGroup",
        json!({ "IdentityStoreId": STORE, "GroupId": "not-a-resource-id!!" }),
    )
    .await;
    assert_eq!(
        (status, code.ends_with("ValidationException")),
        (400, true),
        "expected the modelled constraint to refuse the value with 400 ValidationException, \
         got {status} {code} with {body}"
    );

    // An operation nothing has implemented yet is still the scaffold's 501, not a modelled
    // client fault: the status of a class is not decided by the operation that answered.
    let (status, code, body) = call(
        &router,
        "DescribeIdentityStore",
        json!({ "IdentityStoreId": STORE }),
    )
    .await;
    assert_eq!(
        (status, code.ends_with("NotImplemented")),
        (501, true),
        "expected the scaffold signal to stay 501 NotImplemented, got {status} {code} \
         with {body}"
    );
}

/// The id a response reports is the one a later read reports: a `RequestId` is minted for
/// a FAILURE and never stored on the resource, so the resources a successful call wrote
/// are unchanged by the refusals around them.
#[tokio::test]
async fn minting_a_request_id_writes_nothing() {
    let router = emulator();
    let group = create_group(&router, STORE, "the group to read back", "round trip").await;
    let before = ok(
        &router,
        "DescribeGroup",
        json!({ "IdentityStoreId": STORE, "GroupId": group.clone() }),
        "round trip",
    )
    .await;

    for _ in 0..3 {
        let body = refused(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": UNMINTED_ID }),
            NOT_FOUND,
            "ResourceNotFoundException",
            "round trip",
        )
        .await;
        request_id(&body, "round trip");
    }

    let after = ok(
        &router,
        "DescribeGroup",
        json!({ "IdentityStoreId": STORE, "GroupId": group.clone() }),
        "round trip",
    )
    .await;
    assert_eq!(
        (
            string(&after, "Revision", "round trip"),
            string(&after, "GroupId", "round trip"),
        ),
        (
            string(&before, "Revision", "round trip"),
            string(&before, "GroupId", "round trip"),
        ),
        "expected the refusals to leave the group exactly as it was, got {after}"
    );
    assert!(
        after["RequestId"].is_null(),
        "expected a successful response to carry no RequestId member, got {after}"
    );
}
