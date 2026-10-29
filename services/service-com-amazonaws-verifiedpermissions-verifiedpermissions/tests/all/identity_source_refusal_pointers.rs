// SPDX-License-Identifier: CC0-1.0

//! Which MEMBER a refusal of an identity-source write addresses.
//!
//! The rule under test, one sentence: the JSON pointer a `ValidationException`
//! reports for an offending member — in `fieldList$path` when this service's own
//! checks refused it, and in the refusal message when the modelled constraints
//! did — is rooted at the input member of the OPERATION being refused, as the
//! model names that member for it, so the pointer always addresses a member the
//! refused request really carries.
//!
//! `CreateIdentitySource` and `UpdateIdentitySource` describe one identity
//! provider in two member-identical unions, but they arrive in differently named
//! request members (`configuration` and `updateConfiguration`, from
//! `smithy_query(query="operation", operation="CreateIdentitySource,UpdateIdentitySource")`),
//! so every row below is sent to BOTH operations and expects the SAME pointer
//! under each operation's own root — and the two halves of the crate that can
//! refuse one member (the handler's resolution and the boundary's constraint
//! check) have to agree about it.
//!
//! Every assertion goes over the wire through the shared [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, error_code, fault_message, ok, router, store_with_validation_off, user_pool_arn,
    SIGNING_REGION,
};

/// The Cognito user pool a configuration references, as a pointer WITHIN the
/// member that carries the configuration — the part both writers spell the same
/// way, because `CognitoUserPoolConfiguration` and
/// `UpdateCognitoUserPoolConfiguration` name their members identically
/// (`smithy_query(query="shape", shape="Configuration,UpdateConfiguration")`).
pub const POOL_ARN_IN_CONFIGURATION: &str = "/cognitoUserPoolConfiguration/userPoolArn";

/// An OIDC provider's issuer, as a pointer within the member that carries the
/// configuration — the other arm of the same union.
const ISSUER_IN_CONFIGURATION: &str = "/openIdConnectConfiguration/issuer";

/// The entity type a Cognito user pool's groups map to, as a pointer within the
/// member that carries the configuration — three levels down, and named the same
/// way by `CognitoGroupConfiguration` and `UpdateCognitoGroupConfiguration`.
const GROUP_ENTITY_TYPE_IN_CONFIGURATION: &str =
    "/cognitoUserPoolConfiguration/groupConfiguration/groupEntityType";

/// `CreateIdentitySourceInput`'s provider member.
const CONFIGURATION: &str = "/configuration";

/// `UpdateIdentitySourceInput`'s provider member.
const UPDATE_CONFIGURATION: &str = "/updateConfiguration";

/// `UserPoolArn`'s modelled length maximum, from
/// `smithy_query(query="shape", shape="UpdateCognitoUserPoolConfiguration")`.
const USER_POOL_ARN_MAX_LENGTH: usize = 255;

/// The entity type every row names, so no row is refused for an absent
/// `principalEntityType` instead of the member it is about.
const PRINCIPAL_ENTITY_TYPE: &str = "MyCorp::User";

/// A pool ARN this emulator resolves: its one account, the region the harness
/// signs for, and a pool id naming that same region.
fn reachable_pool_arn() -> String {
    user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_1a2b3c4d5"))
}

/// A well-formed pool ARN padded to `bytes` characters through its POOL ID, so
/// the only constraint such a value can break is the length maximum.
fn pool_arn_of(bytes: usize) -> String {
    let base = user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_"));
    format!("{base}{}", "a".repeat(bytes.saturating_sub(base.len())))
}

/// The pointer a refusal addresses: `fieldList`'s first entry when the service's
/// own checks refused the member, otherwise the member the boundary refusal's
/// message names (`Value at '<pointer>' failed to satisfy constraint: …`).
///
/// One reader for both halves is the point: a client reading either has to be
/// pointed at the same member.
fn refused_pointer(body: &Value) -> String {
    if let Some(path) = body["fieldList"][0]["path"].as_str() {
        return path.to_string();
    }
    fault_message(body)
        .split_once("Value at '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(pointer, _)| pointer.to_string())
        .unwrap_or_default()
}

/// Call `operation` with `provider` as its configuration member and return
/// `(status, body)`.
async fn refuse(
    router: &MultiServiceRouter,
    operation: &str,
    store_id: &str,
    source_id: &str,
    provider: &Value,
) -> (u16, Value) {
    let body = match operation {
        "CreateIdentitySource" => json!({
            "policyStoreId": store_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "configuration": provider,
        }),
        _ => json!({
            "policyStoreId": store_id,
            "identitySourceId": source_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "updateConfiguration": provider,
        }),
    };
    call(router, operation, &body).await
}

/// RULE: a refused member of an identity-source write is addressed at the input
/// member the refused operation declares — `/configuration/…` on the create,
/// `/updateConfiguration/…` on the update — with the same pointer under that root
/// whichever of the two refused it.
///
/// The rows span WHICH HALF of the crate refuses (this service's own resolution
/// of the provider reference, and the modelled constraints the boundary checks),
/// the arm of the union the offending member sits in (Cognito, OIDC), the
/// constraint class broken (an unresolvable reference, a length maximum, a
/// length minimum, a pattern) and the depth of the member below the
/// configuration (two levels and three).
#[tokio::test]
async fn a_refusal_addresses_the_member_the_refused_operation_declares() {
    let account = engine::emulator_account_id();

    // (case, the pointer the refusal must name WITHIN the configuration, the
    //  provider description both operations carry)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            // This service's own resolution: every resource it holds is in its
            // one account.
            "a pool ARN in another account",
            POOL_ARN_IN_CONFIGURATION,
            json!({ "cognitoUserPoolConfiguration": { "userPoolArn":
                format!("arn:aws:cognito-idp:{SIGNING_REGION}:123456789012:userpool/{SIGNING_REGION}_1a2b3c4d5") } }),
        ),
        (
            "a pool ARN in another region",
            POOL_ARN_IN_CONFIGURATION,
            json!({ "cognitoUserPoolConfiguration": { "userPoolArn":
                user_pool_arn("eu-west-1", "eu-west-1_1a2b3c4d5") } }),
        ),
        (
            // A pool id names the region the pool was created in, so an id that
            // disagrees with its own ARN names no pool.
            "a pool id naming another region",
            POOL_ARN_IN_CONFIGURATION,
            json!({ "cognitoUserPoolConfiguration": { "userPoolArn":
                user_pool_arn(SIGNING_REGION, "eu-west-1_1a2b3c4d5") } }),
        ),
        (
            "a pool ARN in another partition",
            POOL_ARN_IN_CONFIGURATION,
            json!({ "cognitoUserPoolConfiguration": { "userPoolArn":
                format!("arn:aws-us-gov:cognito-idp:{SIGNING_REGION}:{account}:userpool/{SIGNING_REGION}_1a2b3c4d5") } }),
        ),
        (
            // The MODELLED length maximum, on a value that satisfies the pattern:
            // the boundary refuses it, and must name the same member.
            "a pool ARN one character past the modelled maximum",
            POOL_ARN_IN_CONFIGURATION,
            json!({ "cognitoUserPoolConfiguration": { "userPoolArn":
                pool_arn_of(USER_POOL_ARN_MAX_LENGTH + 1) } }),
        ),
        (
            // The modelled PATTERN: an ARN that is no user pool ARN at all.
            "an ARN that names no Cognito user pool",
            POOL_ARN_IN_CONFIGURATION,
            json!({ "cognitoUserPoolConfiguration": { "userPoolArn": "arn:aws:s3:::not-a-pool" } }),
        ),
        (
            // A member THREE levels under the configuration, so the root is
            // followed by the whole path down to it.
            "a group entity type below the modelled length minimum",
            GROUP_ENTITY_TYPE_IN_CONFIGURATION,
            json!({ "cognitoUserPoolConfiguration": {
                "userPoolArn": reachable_pool_arn(),
                "groupConfiguration": { "groupEntityType": "" },
            } }),
        ),
        (
            // The OTHER arm of the same union, so the root is followed by the arm
            // the request carried rather than by one arm's names.
            "an OIDC issuer that is no https URL",
            ISSUER_IN_CONFIGURATION,
            json!({ "openIdConnectConfiguration": {
                "issuer": "http://idp.example.com",
                "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "sub" } },
            } }),
        ),
    ];

    let router = router();
    let store_id = store_with_validation_off(&router).await;
    // One live identity source for the update rows to aim at, so an update is
    // refused for its configuration and not for a source that does not exist.
    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": reachable_pool_arn() } },
        }),
    )
    .await;
    let source_id = created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();

    for (case, within, provider) in cases {
        for (operation, root) in [
            ("CreateIdentitySource", CONFIGURATION),
            ("UpdateIdentitySource", UPDATE_CONFIGURATION),
        ] {
            let expected = format!("{root}{within}");
            let (status, body) =
                refuse(&router, operation, &store_id, &source_id, &provider).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} on {operation}: expected a ValidationException, got {body}"
            );
            let pointer = refused_pointer(&body);
            assert_eq!(
                pointer, expected,
                "case {case:?} on {operation}: expected the refusal to address {expected:?}, got \
                 {pointer:?} in {body}"
            );
        }
    }
}

/// RULE (the same one, on the resource rather than on the wording): a refused
/// write changes nothing, so the identity source the update rows aimed at still
/// reports the provider it was created with.
#[tokio::test]
async fn a_refused_write_leaves_the_identity_source_as_it_was() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let arn = reachable_pool_arn();
    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "principalEntityType": PRINCIPAL_ENTITY_TYPE,
            "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
        }),
    )
    .await;
    let source_id = created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();
    let read = json!({ "policyStoreId": store_id, "identitySourceId": source_id });
    let before = ok(&router, "GetIdentitySource", &read).await;

    let (status, body) = call(
        &router,
        "UpdateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "identitySourceId": source_id,
            "updateConfiguration": { "cognitoUserPoolConfiguration": { "userPoolArn":
                user_pool_arn(SIGNING_REGION, "eu-west-1_1a2b3c4d5") } },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an unresolvable pool reference to be refused, got {status} {body}"
    );

    let after = ok(&router, "GetIdentitySource", &read).await;
    assert_eq!(
        after, before,
        "a refused update must change nothing: expected {before}, got {after}"
    );
}
