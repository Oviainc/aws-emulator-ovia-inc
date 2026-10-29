// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the identity-source half of a policy store:
//! `CreateIdentitySource` and the `GetIdentitySource` that reads it back.
//!
//! Every assertion goes over the wire: an awsJson1_0 `POST /` with
//! `X-Amz-Target: VerifiedPermissions.<Operation>`, dispatched through the same
//! `MultiServiceRouter` the emulator binary serves, over ONE shared engine — so
//! a write made by one request is only visible to the next if it really reached
//! state.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A router serving this service and the Amazon Cognito user-pool service it
/// resolves a `userPoolArn` against, over a fresh shared engine. This suite signs
/// requests for several regions, so it wires its own router rather than taking the
/// harness's.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_verifiedpermissions_verifiedpermissions::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one operation, signed for `region`, and return `(http status, parsed body)`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("VerifiedPermissions.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260101/{region}/verifiedpermissions/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a test request body always serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The region every request in this suite is signed for unless it says otherwise.
const REGION: &str = "us-east-1";

/// Call one operation signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// Call one operation, requiring success, and return the parsed body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} failed: {parsed}");
    parsed
}

/// Create a store with validation mode `OFF` and return its `policyStoreId`.
async fn store(router: &MultiServiceRouter) -> String {
    let created = ok(
        router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": "OFF" } }),
    )
    .await;
    created["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string()
}

/// The error code an awsJson1_0 fault carries in `__type`, without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The ONE pool-ARN helper this test crate has lives in the shared harness; this
/// suite reaches it rather than shaping a second copy, and seeds the pools it
/// references through the harness too ([`super::harness::user_pool`]) — an
/// identity source can only reference a user pool that exists.
use super::harness::user_pool_arn;

/// `IdentitySourceId`'s published shape: `IS` + 22 base62 characters, inside the
/// modelled `^[a-zA-Z0-9-]*$`.
fn looks_like_an_identity_source_id(id: &str) -> bool {
    id.len() == 24
        && id.starts_with("IS")
        && id[2..].chars().all(|c| c.is_ascii_alphanumeric())
}

// ---------------------------------------------------------------------------
// Create -> Get round trip
// ---------------------------------------------------------------------------

/// Every member of an identity source is read back out of state, across the
/// whole range of configurations a create may carry: both arms of the
/// `Configuration` union, both arms of `OpenIdConnectTokenSelection`, with and
/// without the optional client ids, group configuration, entity-id prefix and
/// `principalEntityType`.
#[tokio::test]
async fn identity_source_round_trips_every_configuration_it_was_created_with() {
    let router = router();
    let store_id = store(&router).await;
    let pool = super::harness::user_pool(&router, "round-trip-pool").await;
    let arn = user_pool_arn(REGION, &pool);
    // The issuer and discovery URL the model documents for a Cognito user pool:
    // `https://cognito-idp.<region>.amazonaws.com/<user-pool-id>`, and that URL
    // plus `/.well-known/openid-configuration`.
    let issuer = format!("https://cognito-idp.{REGION}.amazonaws.com/{pool}");
    let discovery = format!("{issuer}/.well-known/openid-configuration");
    let arn = arn.as_str();

    // (case, request members beside policyStoreId, expected principalEntityType,
    //  expected configuration, expected deprecated details)
    let cases: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            // No principalEntityType: the create's optional member resolves to
            // the service's default, because every read requires one.
            "cognito-minimal",
            json!({ "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } } }),
            "AWS::Cognito",
            json!({
                "cognitoUserPoolConfiguration": {
                    "userPoolArn": arn,
                    "clientIds": [],
                    "issuer": issuer,
                }
            }),
            json!({
                "clientIds": [],
                "userPoolArn": arn,
                "discoveryUrl": discovery,
                "openIdIssuer": "COGNITO",
            }),
        ),
        (
            "cognito-clients-groups-and-entity-type",
            json!({
                "configuration": {
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn": arn,
                        "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc", "zyxwvutsrq9876543210"],
                        "groupConfiguration": { "groupEntityType": "MyCorp::Group" },
                    }
                },
                "principalEntityType": "MyCorp::User",
            }),
            "MyCorp::User",
            json!({
                "cognitoUserPoolConfiguration": {
                    "userPoolArn": arn,
                    "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc", "zyxwvutsrq9876543210"],
                    "issuer": issuer,
                    "groupConfiguration": { "groupEntityType": "MyCorp::Group" },
                }
            }),
            json!({
                "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc", "zyxwvutsrq9876543210"],
                "userPoolArn": arn,
                "discoveryUrl": discovery,
                "openIdIssuer": "COGNITO",
            }),
        ),
        (
            "oidc-access-token-only",
            json!({
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://auth.example.com",
                        "tokenSelection": {
                            "accessTokenOnly": {
                                "principalIdClaim": "sub",
                                "audiences": ["https://myapp.example.com"],
                            }
                        },
                    }
                },
                "principalEntityType": "MyCorp::User",
            }),
            "MyCorp::User",
            json!({
                "openIdConnectConfiguration": {
                    "issuer": "https://auth.example.com",
                    "tokenSelection": {
                        "accessTokenOnly": {
                            "principalIdClaim": "sub",
                            "audiences": ["https://myapp.example.com"],
                        }
                    },
                }
            }),
            // `details` describes a Cognito user pool, so an OIDC source has none.
            Value::Null,
        ),
        (
            "oidc-identity-token-with-prefix-and-groups",
            json!({
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://oidc.example.org/tenant",
                        "entityIdPrefix": "MyOIDCProvider",
                        "groupConfiguration": {
                            "groupClaim": "groups",
                            "groupEntityType": "MyCorp::UserGroup",
                        },
                        "tokenSelection": {
                            "identityTokenOnly": {
                                "principalIdClaim": "sub",
                                "clientIds": ["clientone", "clienttwo"],
                            }
                        },
                    }
                },
                "principalEntityType": "MyCorp::Person",
            }),
            "MyCorp::Person",
            json!({
                "openIdConnectConfiguration": {
                    "issuer": "https://oidc.example.org/tenant",
                    "entityIdPrefix": "MyOIDCProvider",
                    "groupConfiguration": {
                        "groupClaim": "groups",
                        "groupEntityType": "MyCorp::UserGroup",
                    },
                    "tokenSelection": {
                        "identityTokenOnly": {
                            "principalIdClaim": "sub",
                            "clientIds": ["clientone", "clienttwo"],
                        }
                    },
                }
            }),
            Value::Null,
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, members, principal_entity_type, configuration, details) in cases {
        let mut request = members;
        request["policyStoreId"] = json!(store_id);
        let created = ok(&router, "CreateIdentitySource", &request).await;

        let id = created["identitySourceId"].as_str().unwrap_or_default();
        assert!(
            looks_like_an_identity_source_id(id),
            "case {case:?}: expected an IS-prefixed identity source id, got {id:?}"
        );
        assert!(
            !seen.contains(&id.to_string()),
            "case {case:?}: expected a distinct identity source id, got the already-minted {id:?}"
        );
        seen.push(id.to_string());
        assert_eq!(
            created["policyStoreId"], json!(store_id),
            "case {case:?}: expected the response to name the store it was created in, got {:?}",
            created["policyStoreId"]
        );
        assert_eq!(
            created["createdDate"], created["lastUpdatedDate"],
            "case {case:?}: a just-created identity source was never updated, got {created:?}"
        );

        let got = ok(
            &router,
            "GetIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": id }),
        )
        .await;
        assert_eq!(
            got["identitySourceId"], json!(id),
            "case {case:?}: expected the read to name the identity source asked for, got {:?}",
            got["identitySourceId"]
        );
        assert_eq!(
            got["policyStoreId"], json!(store_id),
            "case {case:?}: expected the read to name the store, got {:?}",
            got["policyStoreId"]
        );
        assert_eq!(
            got["principalEntityType"], json!(principal_entity_type),
            "case {case:?}: expected principalEntityType {principal_entity_type:?}, got {:?}",
            got["principalEntityType"]
        );
        assert_eq!(
            got["configuration"], configuration,
            "case {case:?}: expected configuration {configuration}, got {:?}",
            got["configuration"]
        );
        assert_eq!(
            got["details"], details,
            "case {case:?}: expected details {details}, got {:?}",
            got["details"]
        );
        assert_eq!(
            got["createdDate"], created["createdDate"],
            "case {case:?}: expected the stored createdDate {:?}, got {:?}",
            created["createdDate"], got["createdDate"]
        );
        assert_eq!(
            got["lastUpdatedDate"], created["lastUpdatedDate"],
            "case {case:?}: expected the stored lastUpdatedDate {:?}, got {:?}",
            created["lastUpdatedDate"], got["lastUpdatedDate"]
        );
    }
}

// ---------------------------------------------------------------------------
// A provider reference the service cannot resolve
// ---------------------------------------------------------------------------

/// A `userPoolArn` naming a pool this emulator does not hold is refused, and
/// persists nothing. The rows differ along WHICH segment of the ARN puts the
/// pool out of reach: the account, the ARN's region, and the region the pool id
/// itself names.
#[tokio::test]
async fn a_user_pool_this_emulator_cannot_resolve_is_refused() {
    let cases: Vec<(&str, String)> = vec![
        (
            // Every resource this emulator holds is in `emulator_account_id()`.
            "another-account",
            "arn:aws:cognito-idp:us-east-1:123456789012:userpool/us-east-1_1a2b3c4d5".to_string(),
        ),
        (
            // A pool has to be in the policy store's own region.
            "another-region",
            user_pool_arn("eu-west-1", "eu-west-1_1a2b3c4d5"),
        ),
        (
            // A user pool id names the region the pool was created in, so an id
            // that disagrees with its own ARN names no pool at all.
            "pool-id-region-disagrees-with-the-arn",
            user_pool_arn(REGION, "eu-west-1_1a2b3c4d5"),
        ),
        (
            // A pool in another partition is not reachable from this one, even
            // with this emulator's own account and the request's own region.
            "another-partition",
            "arn:aws-us-gov:cognito-idp:us-east-1:000000000000:userpool/us-east-1_1a2b3c4d5"
                .to_string(),
        ),
        (
            "another-partition-and-account",
            "arn:aws-us-gov:cognito-idp:us-east-1:210987654321:userpool/us-east-1_9z8y7x6w5"
                .to_string(),
        ),
    ];

    let router = router();
    let store_id = store(&router).await;
    // The pool the fallback row below references really exists, so the only reason
    // a row is refused is the segment of ITS ARN that row changed.
    let live_pool_arn =
        user_pool_arn(REGION, &super::harness::user_pool(&router, "live-pool").await);
    for (case, arn) in cases {
        let (status, body) = call(
            &router,
            "CreateIdentitySource",
            &json!({
                "policyStoreId": store_id,
                "clientToken": format!("token-{case}"),
                "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {arn}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException for {arn}, got {body}"
        );

        // Nothing was persisted — not even the client token, which a refused
        // request must leave unrecorded: re-using it with other parameters
        // succeeds instead of conflicting.
        let replayed = ok(
            &router,
            "CreateIdentitySource",
            &json!({
                "policyStoreId": store_id,
                "clientToken": format!("token-{case}"),
                "configuration": {
                    "cognitoUserPoolConfiguration": { "userPoolArn": live_pool_arn },
                },
            }),
        )
        .await;
        assert!(
            looks_like_an_identity_source_id(
                replayed["identitySourceId"].as_str().unwrap_or_default()
            ),
            "case {case:?}: a refused create must record no client token, got {replayed}"
        );
    }
}

/// Which pools resolve follows the region the REQUEST is signed for, not one
/// fixed region: the same ARN is accepted in its own region and refused from
/// another.
#[tokio::test]
async fn the_signed_region_decides_which_user_pools_resolve() {
    // (case, signing region, the region the pool is created in, expected status)
    let cases: &[(&str, &str, &str, u16)] = &[
        ("home-region", "us-east-1", "us-east-1", 200),
        ("other-home-region", "eu-west-1", "eu-west-1", 200),
        ("ap-home-region", "ap-southeast-2", "ap-southeast-2", 200),
        ("cross-region", "eu-west-1", "us-east-1", 400),
        ("cross-region-reversed", "us-east-1", "ap-southeast-2", 400),
    ];

    let router = router();
    let store_id = store(&router).await;
    for (case, signing_region, pool_region, expected) in cases {
        // Each row's pool is seeded in the region the row names, so its id names
        // that region and the pool really exists there.
        let pool = super::harness::user_pool_in(&router, pool_region, &format!("pool-{case}")).await;
        let arn = user_pool_arn(pool_region, &pool);
        let (status, body) = call_in(
            &router,
            signing_region,
            "CreateIdentitySource",
            &json!({
                "policyStoreId": store_id,
                "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected} for {arn} signed for {signing_region}, \
             got {status} {body}"
        );
        if *expected == 200 {
            // The issuer is derived from the pool's own region, whichever it is.
            let id = body["identitySourceId"].as_str().unwrap_or_default();
            let got = call_in(
                &router,
                signing_region,
                "GetIdentitySource",
                &json!({ "policyStoreId": store_id, "identitySourceId": id }),
            )
            .await
            .1;
            let expected_issuer = format!("https://cognito-idp.{pool_region}.amazonaws.com/{pool}");
            assert_eq!(
                got["configuration"]["cognitoUserPoolConfiguration"]["issuer"],
                json!(expected_issuer),
                "case {case:?}: expected issuer {expected_issuer}, got {got}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException for {arn}, got {body}"
            );
        }
    }
}

/// An OIDC identity source generates principals of an entity type only the
/// caller can name, so a create that omits `principalEntityType` for one is
/// refused rather than given a Cognito default.
#[tokio::test]
async fn an_oidc_identity_source_needs_a_principal_entity_type() {
    let router = router();
    let store_id = store(&router).await;
    let configuration = json!({
        "openIdConnectConfiguration": {
            "issuer": "https://auth.example.com",
            "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
        }
    });

    let (status, body) = call(
        &router,
        "CreateIdentitySource",
        &json!({ "policyStoreId": store_id, "configuration": configuration }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"oidc-without-principal-entity-type\": expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"oidc-without-principal-entity-type\": expected a ValidationException, got {body}"
    );

    // The same configuration WITH the entity type is accepted.
    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "configuration": configuration,
            "principalEntityType": "MyCorp::User",
        }),
    )
    .await;
    assert!(
        looks_like_an_identity_source_id(created["identitySourceId"].as_str().unwrap_or_default()),
        "case \"oidc-with-principal-entity-type\": expected an identity source id, got {created}"
    );
}

// ---------------------------------------------------------------------------
// Resources that do not exist
// ---------------------------------------------------------------------------

/// A reference to a policy store or an identity source that does not exist is a
/// 404 `ResourceNotFoundException` naming the resource that was not found —
/// including an identity source read through a DIFFERENT store, because an
/// identity source belongs to the store it was created in.
#[tokio::test]
async fn an_unknown_policy_store_or_identity_source_is_not_found() {
    let router = router();
    let store_id = store(&router).await;
    let other_store_id = store(&router).await;
    let pool_arn = user_pool_arn(REGION, &super::harness::user_pool(&router, "not-found-pool").await);
    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": pool_arn } },
        }),
    )
    .await;
    let id = created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();

    // (case, operation, request, expected resourceType)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "create-in-an-unknown-store",
            "CreateIdentitySource",
            json!({
                "policyStoreId": "PSdoesnotexist11111111",
                "configuration": {
                    "cognitoUserPoolConfiguration": { "userPoolArn": pool_arn },
                },
            }),
            "POLICY_STORE",
        ),
        (
            "create-under-an-unknown-alias",
            "CreateIdentitySource",
            json!({
                "policyStoreId": "policy-store-alias/not-an-alias",
                "configuration": {
                    "cognitoUserPoolConfiguration": { "userPoolArn": pool_arn },
                },
            }),
            // The reference named an ALIAS, so the alias is the resource reported
            // missing — `ResourceNotFoundException$resourceType` is "the resource
            // type of the resource referenced in the failed request".
            "POLICY_STORE_ALIAS",
        ),
        (
            "get-from-an-unknown-store",
            "GetIdentitySource",
            json!({ "policyStoreId": "PSdoesnotexist11111111", "identitySourceId": id }),
            "POLICY_STORE",
        ),
        (
            "get-an-unknown-identity-source",
            "GetIdentitySource",
            json!({ "policyStoreId": store_id, "identitySourceId": "ISdoesnotexist11111111" }),
            "IDENTITY_SOURCE",
        ),
        (
            "get-through-another-store",
            "GetIdentitySource",
            json!({ "policyStoreId": other_store_id, "identitySourceId": id }),
            "IDENTITY_SOURCE",
        ),
    ];

    for (case, operation, request, resource_type) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceType"], json!(resource_type),
            "case {case:?}: expected resourceType {resource_type:?}, got {:?}",
            body["resourceType"]
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------
/// A replayed `clientToken` answers with the identity source the first call
/// created; the same token with different parameters conflicts. The rows differ
/// along WHICH parameter changed.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_identity_source_it_created() {
    let router = router();
    let store_id = store(&router).await;
    let arn = user_pool_arn(REGION, &super::harness::user_pool(&router, "replay-pool").await);
    // The row that changes the pool names a SECOND pool that also exists, so what
    // the conflict is about is the changed parameter and not an unresolvable ARN.
    let other_arn =
        user_pool_arn(REGION, &super::harness::user_pool(&router, "replay-other-pool").await);
    let first = json!({
        "policyStoreId": store_id,
        "clientToken": "11111111-2222-3333-4444-555555555555",
        "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
        "principalEntityType": "MyCorp::User",
    });

    let created = ok(&router, "CreateIdentitySource", &first).await;
    let id = created["identitySourceId"].as_str().unwrap_or_default();
    let replayed = ok(&router, "CreateIdentitySource", &first).await;
    assert_eq!(
        replayed, created,
        "case \"same-token-same-parameters\": expected the first result {created}, got {replayed}"
    );

    // Each row changes exactly one parameter under the same token.
    let conflicts: Vec<(&str, Value)> = vec![
        (
            "principal-entity-type-changed",
            json!({
                "policyStoreId": store_id,
                "clientToken": "11111111-2222-3333-4444-555555555555",
                "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
                "principalEntityType": "MyCorp::Person",
            }),
        ),
        (
            "user-pool-changed",
            json!({
                "policyStoreId": store_id,
                "clientToken": "11111111-2222-3333-4444-555555555555",
                "configuration": {
                    "cognitoUserPoolConfiguration": { "userPoolArn": other_arn },
                },
                "principalEntityType": "MyCorp::User",
            }),
        ),
        (
            "client-ids-added",
            json!({
                "policyStoreId": store_id,
                "clientToken": "11111111-2222-3333-4444-555555555555",
                "configuration": {
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn": arn,
                        "clientIds": ["a1b2c3d4e5f6g7h8i9j0kalbmc"],
                    }
                },
                "principalEntityType": "MyCorp::User",
            }),
        ),
        (
            "provider-kind-changed",
            json!({
                "policyStoreId": store_id,
                "clientToken": "11111111-2222-3333-4444-555555555555",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://auth.example.com",
                        "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
                    }
                },
                "principalEntityType": "MyCorp::User",
            }),
        ),
    ];

    for (case, request) in conflicts {
        let (status, body) = call(&router, "CreateIdentitySource", &request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected a ConflictException, got {body}"
        );
        assert_eq!(
            body["resources"],
            json!([{ "resourceId": id, "resourceType": "IDENTITY_SOURCE" }]),
            "case {case:?}: expected the conflict to name {id}, got {:?}",
            body["resources"]
        );
    }

    // A different token creates a second, distinct identity source.
    let other = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "clientToken": "66666666-7777-8888-9999-000000000000",
            "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
            "principalEntityType": "MyCorp::User",
        }),
    )
    .await;
    assert_ne!(
        other["identitySourceId"], created["identitySourceId"],
        "case \"another-token\": expected a distinct identity source, got {other}"
    );
}

// ---------------------------------------------------------------------------
// The per-store bound
// ---------------------------------------------------------------------------

/// A store at its identity-source bound refuses the next create with the
/// modelled 402 `ServiceQuotaExceededException`, and the sources it already
/// holds are still readable. Two rows: the create that reaches the bound, and
/// the one past it.
#[tokio::test]
async fn a_store_at_its_identity_source_bound_refuses_the_next_one() {
    // The bound this crate publishes, so the test follows the constant rather
    // than a copy of its value.
    let bound = service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::MAX_IDENTITY_SOURCES_PER_STORE;

    let router = router();
    let store_id = store(&router).await;
    let mut first_id = String::new();
    for index in 0..bound {
        // Each source references a pool of its own, every one of them seeded, so
        // the only thing that stops the next create is the bound.
        let pool = super::harness::user_pool(&router, &format!("bound-pool{index}")).await;
        let created = ok(
            &router,
            "CreateIdentitySource",
            &json!({
                "policyStoreId": store_id,
                "configuration": {
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn": user_pool_arn(REGION, &pool),
                    }
                },
            }),
        )
        .await;
        if index == 0 {
            first_id = created["identitySourceId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
    }

    let one_too_many = super::harness::user_pool(&router, "bound-pool-one-too-many").await;
    let (status, body) = call(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "configuration": {
                "cognitoUserPoolConfiguration": {
                    "userPoolArn": user_pool_arn(REGION, &one_too_many),
                }
            },
        }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"one-past-the-bound\": expected HTTP 402 after {bound} identity sources, \
         got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"one-past-the-bound\": expected a ServiceQuotaExceededException, got {body}"
    );
    assert_eq!(
        body["resourceType"], json!("IDENTITY_SOURCE"),
        "case \"one-past-the-bound\": expected resourceType IDENTITY_SOURCE, got {:?}",
        body["resourceType"]
    );
    assert_eq!(
        body["resourceId"], json!(store_id),
        "case \"one-past-the-bound\": expected the refusal to name the store, got {:?}",
        body["resourceId"]
    );

    // The refusal changed nothing: what the store already holds still reads back.
    let got = ok(
        &router,
        "GetIdentitySource",
        &json!({ "policyStoreId": store_id, "identitySourceId": first_id }),
    )
    .await;
    assert_eq!(
        got["identitySourceId"], json!(first_id),
        "case \"at-the-bound\": expected the first identity source to still read back, got {got}"
    );
}
