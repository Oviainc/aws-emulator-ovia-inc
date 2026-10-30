// SPDX-License-Identifier: CC0-1.0

//! What a REFUSAL of this service looks like on the wire: the HTTP status it travels with,
//! and the exception shape it names in the `x-amzn-errortype` header.
//!
//! Two rules, each asserted over a table of inputs rather than over one recorded call:
//!
//! * a refusal that says the resource the request names does not exist travels with this
//!   service's client-error status (HTTP 400,
//!   `service_..._awscognitoidentityservice::scaffolded::CLIENT_ERROR_STATUS`) and NOT with
//!   the 404 the shape's `@httpError` annotation carries
//!   (`smithy_query(query="operation",operation="DescribeIdentityPool")`:
//!   `ResourceNotFoundException` `errorKind=client`, `httpError=404`) — on this `awsJson1_1`
//!   service the 404 belongs to the transport, so no modelled refusal travels with it. The
//!   rows span the operations that can refuse that way and both kinds of resource one of
//!   them names, a pool and an identity, because the rule is about the refusal and not
//!   about any one operation;
//! * every failure, whichever layer answered it, names its exception shape in the
//!   `x-amzn-errortype` header, as the shape's bare name followed by the separator before
//!   the (empty) locator: `ResourceNotFoundException:`. The rows span the layers — a
//!   handler's modelled error, a constraint refusal raised before any handler ran, and a
//!   failure no operation produced — and the error classes, so a refusal of another class
//!   is seen to keep its own status while naming its own shape.
//!
//! The harness is `response_request_id`'s, because both rules are about response HEADERS
//! and `identity_pool_flow::Emulator` reports a status and a parsed body; it drives the
//! same `MultiServiceRouter` over the same single shared engine.

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::scaffolded::CLIENT_ERROR_STATUS;
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::identity_pool_arn;

use crate::identity_pool_flow::ABSENT_POOL_ID;
use crate::response_request_id::{Caller, Emulator};

/// The header a failure names its exception shape in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The exception shape a refusal names when the resource the request addressed is not
/// there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A second well-formed `IdentityPoolId` nothing created, in another Region: the rule is
/// about the pool being absent, not about which id form it has.
const ABSENT_POOL_ID_OTHER_REGION: &str = "eu-west-2:00000000-0000-0000-0000-00000000beef";

/// Well-formed `IdentityId`s that no `GetId` ever minted — `IdentityId` is modelled
/// `len 1..55, pattern ^[\w-]+:[0-9a-f-]+$`, so one has the usual REGION:GUID form and one
/// sits at the shortest form that pattern admits.
const ABSENT_IDENTITY_ID: &str = "us-east-1:22222222-2222-2222-2222-222222222222";
const SHORTEST_ABSENT_IDENTITY_ID: &str = "a:0";

/// An IAM role ARN a role configuration can carry, so a row that names an absent pool is
/// refused for the pool rather than for the role.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// One answered call: its status, its parsed body and the exception shape its header names.
struct Answer {
    status: u16,
    body: Value,
    error_type: Option<String>,
}

impl Answer {
    /// The exception shape the BODY names (`__type` on the awsJson protocols), reduced to
    /// the bare shape name, or `None` when the body names none.
    fn body_shape(&self) -> Option<String> {
        let raw = self.body.get("__type")?.as_str()?;
        let bare = raw.rsplit('#').next().unwrap_or(raw);
        Some(bare.split(':').next().unwrap_or(bare).to_string())
    }
}

/// Answer one call through the router, reporting everything the two rules are about.
async fn answer(emulator: &Emulator, caller: Caller, operation: &str, body: &Value) -> Answer {
    let response = emulator.dispatch(caller, operation, body).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(|value| {
            value
                .to_str()
                .expect("the error-type header is ASCII")
                .to_string()
        });
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        body,
        error_type,
    }
}

/// The header value a response naming `shape` carries.
fn error_type_of(shape: &str) -> String {
    format!("{shape}:")
}

/// One row of the layer-and-class table: how the call is addressed, the exception shape it
/// must name, and the status its error class carries — `None` where the status is another
/// rule's concern.
struct Refusal {
    case: &'static str,
    caller: Caller,
    operation: &'static str,
    request: Value,
    shape: &'static str,
    status: Option<u16>,
}

/// RULE: a refusal that says the resource the request names does not exist answers this
/// service's client-error status, names `ResourceNotFoundException` in its body and names
/// that shape in the error-type header — for every operation that can refuse that way, and
/// for a pool as much as for an identity.
#[tokio::test]
async fn a_resource_that_is_not_there_is_refused_with_the_client_error_status() {
    let emulator = Emulator::new(false);

    // The rows differ along the operation, along which resource the request addresses (a
    // pool by id, a pool by ARN, an identity) and along the form of the absent id, which is
    // what the rule generalises over. None of them is credentialed differently from the way
    // the model marks it: the credential exchange is called unsigned.
    let rows: Vec<(&str, Caller, &str, Value)> = vec![
        (
            "a delete of a pool nothing created",
            Caller::Signed,
            "DeleteIdentityPool",
            json!({ "IdentityPoolId": ABSENT_POOL_ID }),
        ),
        (
            "a read of a pool nothing created, in another Region",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": ABSENT_POOL_ID_OTHER_REGION }),
        ),
        (
            "an update of a pool nothing created",
            Caller::Signed,
            "UpdateIdentityPool",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "IdentityPoolName": "renamed-by-a-doomed-update",
                "AllowUnauthenticatedIdentities": true,
            }),
        ),
        (
            "a listing of the identities of a pool nothing created",
            Caller::Signed,
            "ListIdentities",
            json!({ "IdentityPoolId": ABSENT_POOL_ID, "MaxResults": 10 }),
        ),
        (
            "a role read of a pool nothing created",
            Caller::Signed,
            "GetIdentityPoolRoles",
            json!({ "IdentityPoolId": ABSENT_POOL_ID }),
        ),
        (
            "a role write on a pool nothing created",
            Caller::Signed,
            "SetIdentityPoolRoles",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "Roles": { "authenticated": role_arn("Cognito_absentAuth_Role") },
            }),
        ),
        (
            "a principal-tag read of a pool nothing created",
            Caller::Signed,
            "GetPrincipalTagAttributeMap",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "IdentityProviderName": "graph.facebook.com",
            }),
        ),
        (
            "a principal-tag write on a pool nothing created",
            Caller::Signed,
            "SetPrincipalTagAttributeMap",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "IdentityProviderName": "graph.facebook.com",
                "UseDefaults": true,
            }),
        ),
        (
            "a tag read of the ARN of a pool nothing created",
            Caller::Signed,
            "ListTagsForResource",
            json!({ "ResourceArn": identity_pool_arn(ABSENT_POOL_ID) }),
        ),
        (
            "a tag write on the ARN of a pool nothing created",
            Caller::Signed,
            "TagResource",
            json!({
                "ResourceArn": identity_pool_arn(ABSENT_POOL_ID),
                "Tags": { "Environment": "Test" },
            }),
        ),
        (
            "a tag removal on the ARN of a pool nothing created",
            Caller::Signed,
            "UntagResource",
            json!({
                "ResourceArn": identity_pool_arn(ABSENT_POOL_ID),
                "TagKeys": ["Environment"],
            }),
        ),
        (
            "a developer-identity lookup in a pool nothing created",
            Caller::Signed,
            "LookupDeveloperIdentity",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "DeveloperUserIdentifier": "nobody",
                "MaxResults": 1,
            }),
        ),
        (
            "a developer-identity merge in a pool nothing created",
            Caller::Signed,
            "MergeDeveloperIdentities",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "SourceUserIdentifier": "source-user",
                "DestinationUserIdentifier": "destination-user",
                "DeveloperProviderName": "login.example.app",
            }),
        ),
        (
            "a developer-identity token for a pool nothing created",
            Caller::Signed,
            "GetOpenIdTokenForDeveloperIdentity",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "Logins": { "login.example.app": "a-backend-user" },
            }),
        ),
        (
            "an identity minted in a pool nothing created, called unsigned",
            Caller::Anonymous,
            "GetId",
            json!({ "IdentityPoolId": ABSENT_POOL_ID }),
        ),
        (
            "a read of an identity nothing minted",
            Caller::Signed,
            "DescribeIdentity",
            json!({ "IdentityId": ABSENT_IDENTITY_ID }),
        ),
        (
            "a token for an identity nothing minted, called unsigned",
            Caller::Anonymous,
            "GetOpenIdToken",
            json!({ "IdentityId": ABSENT_IDENTITY_ID }),
        ),
        (
            "credentials for an identity at the shortest id the pattern admits",
            Caller::Anonymous,
            "GetCredentialsForIdentity",
            json!({ "IdentityId": SHORTEST_ABSENT_IDENTITY_ID }),
        ),
        (
            "a login removal from an identity nothing minted",
            Caller::Signed,
            "UnlinkIdentity",
            json!({
                "IdentityId": ABSENT_IDENTITY_ID,
                "Logins": { "graph.facebook.com": "a-facebook-access-token" },
                "LoginsToRemove": ["graph.facebook.com"],
            }),
        ),
        (
            "a developer-login removal from an identity nothing minted",
            Caller::Signed,
            "UnlinkDeveloperIdentity",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "IdentityId": ABSENT_IDENTITY_ID,
                "DeveloperProviderName": "login.example.app",
                "DeveloperUserIdentifier": "nobody",
            }),
        ),
    ];

    for (case, caller, operation, request) in rows {
        let answered = answer(&emulator, caller, operation, &request).await;
        assert_eq!(
            answered.status, CLIENT_ERROR_STATUS,
            "case {case:?} on {operation}: expected HTTP {CLIENT_ERROR_STATUS}, got {} with {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.body_shape().as_deref(),
            Some(NOT_FOUND),
            "case {case:?} on {operation}: expected the body to name {NOT_FOUND}, got {}",
            answered.body
        );
        assert_eq!(
            answered.error_type.as_deref(),
            Some(error_type_of(NOT_FOUND).as_str()),
            "case {case:?} on {operation}: expected the header to name {NOT_FOUND}, got {:?}",
            answered.error_type
        );
    }
}

/// RULE: every failure names its exception shape in the error-type header, whichever layer
/// answered it — and a refusal of another class keeps the status its own class carries, so
/// the client-error status above is the not-found class's and not a status every refusal is
/// flattened onto.
#[tokio::test]
async fn every_failure_names_its_exception_shape_in_the_error_type_header() {
    let emulator = Emulator::new(false);
    // A pool that forbids unauthenticated identities, so an unsigned `GetId` carrying no
    // login is an access decision rather than a missing resource.
    let closed_pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "wire-error-envelope-closed",
                "AllowUnauthenticatedIdentities": false,
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();

    // The rows differ along the LAYER that answers and the error CLASS it answers with:
    // the boundary validator ahead of every handler, a handler's own bad-parameter and
    // access refusals, a handler's not-found, and a failure no operation produced at all.
    // A row states the status its class carries, except the last: the status a request
    // naming no modelled operation answers with is the routing layer's concern, not this
    // rule's.
    let rows = vec![
        Refusal {
            case: "a value outside the pattern the model states",
            caller: Caller::Signed,
            operation: "DescribeIdentityPool",
            request: json!({ "IdentityPoolId": "not-an-identity-pool-id" }),
            shape: "ValidationException",
            status: Some(400),
        },
        Refusal {
            case: "a member the model requires and the request omits",
            caller: Caller::Signed,
            operation: "CreateIdentityPool",
            request: json!({ "IdentityPoolName": "no-unauthenticated-flag" }),
            shape: "ValidationException",
            status: Some(400),
        },
        Refusal {
            case: "an ARN that is no identity-pool ARN of this service",
            caller: Caller::Signed,
            operation: "ListTagsForResource",
            request: json!({ "ResourceArn": "arn:aws:sns:us-east-1:000000000000:a-topic-name" }),
            shape: "InvalidParameterException",
            status: Some(400),
        },
        Refusal {
            case: "an unsigned identity request on a pool that forbids unauthenticated ones",
            caller: Caller::Anonymous,
            operation: "GetId",
            request: json!({ "IdentityPoolId": closed_pool_id }),
            shape: "NotAuthorizedException",
            status: Some(CLIENT_ERROR_STATUS),
        },
        Refusal {
            case: "a resource that is not there",
            caller: Caller::Signed,
            operation: "DeleteIdentityPool",
            request: json!({ "IdentityPoolId": ABSENT_POOL_ID }),
            shape: NOT_FOUND,
            status: Some(CLIENT_ERROR_STATUS),
        },
        Refusal {
            case: "a request naming no modelled operation",
            caller: Caller::Signed,
            operation: "NoSuchOperationInThisModel",
            request: json!({}),
            shape: "NotImplemented",
            status: None,
        },
    ];

    for row in rows {
        let Refusal {
            case,
            caller,
            operation,
            request,
            shape,
            status: expected_status,
        } = row;
        let answered = answer(&emulator, caller, operation, &request).await;
        if let Some(expected) = expected_status {
            assert_eq!(
                answered.status, expected,
                "case {case:?}: expected HTTP {expected}, got {} with {}",
                answered.status, answered.body
            );
        }
        assert!(
            answered.status >= 400,
            "case {case:?}: expected a failure, got {} with {}",
            answered.status,
            answered.body
        );
        assert_eq!(
            answered.body_shape().as_deref(),
            Some(shape),
            "case {case:?}: expected the body to name {shape}, got {}",
            answered.body
        );
        assert_eq!(
            answered.error_type.as_deref(),
            Some(error_type_of(shape).as_str()),
            "case {case:?}: expected the header to name {shape}, got {:?}",
            answered.error_type
        );
    }
}

/// RULE: a SUCCESS names no exception shape — the header belongs to a failure alone, so
/// nothing a client reads it as is attached to a response that refused nothing.
#[tokio::test]
async fn a_success_names_no_exception_shape() {
    let emulator = Emulator::new(false);
    let pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "wire-error-envelope-open",
                "AllowUnauthenticatedIdentities": true,
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();

    // The rows differ along the SHAPE of the success: a report, a listing, a report of an
    // empty collection, a mutation answering an empty body, and an unsigned call.
    let rows: Vec<(&str, Caller, &str, Value)> = vec![
        (
            "a read",
            Caller::Signed,
            "DescribeIdentityPool",
            json!({ "IdentityPoolId": &pool_id }),
        ),
        (
            "a listing",
            Caller::Signed,
            "ListIdentityPools",
            json!({ "MaxResults": 10 }),
        ),
        (
            "a report of an empty collection",
            Caller::Signed,
            "ListTagsForResource",
            json!({ "ResourceArn": identity_pool_arn(&pool_id) }),
        ),
        (
            "a mutation answering an empty body",
            Caller::Signed,
            "TagResource",
            json!({
                "ResourceArn": identity_pool_arn(&pool_id),
                "Tags": { "Environment": "Test" },
            }),
        ),
        (
            "an unsigned call",
            Caller::Anonymous,
            "GetId",
            json!({ "IdentityPoolId": &pool_id }),
        ),
    ];

    for (case, caller, operation, request) in rows {
        let answered = answer(&emulator, caller, operation, &request).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?} on {operation}: expected HTTP 200, got {} with {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error_type, None,
            "case {case:?} on {operation}: expected no {ERROR_TYPE_HEADER}, got {:?}",
            answered.error_type
        );
    }
}
