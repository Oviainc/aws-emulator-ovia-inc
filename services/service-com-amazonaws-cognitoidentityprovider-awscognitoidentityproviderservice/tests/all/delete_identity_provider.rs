// SPDX-License-Identifier: CC0-1.0

//! `DeleteIdentityProvider` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `DeleteIdentityProvider` removes, from the pool named by
//! `UserPoolId`, the stored IdP its exact `ProviderName` names — with the IdP
//! identifiers that provider carried — and drops that name from the
//! `SupportedIdentityProviders` of every app client of the pool that listed it,
//! because "after you delete an IdP, users can no longer sign in to your user
//! pool through that IdP" (this operation's model documentation). Every other
//! provider, every other entry of a client's list, and every other pool is left
//! untouched. A pool the request does not name, or a provider the named pool
//! does not hold (a second delete of the same provider included), is a 404
//! `ResourceNotFoundException` that persists nothing, worded as every operation
//! that resolves an IdP against a pool words it: it names what was asked for and
//! the pool it was looked up in.
//!
//! The deletion is read back through the implemented readers of the same state:
//! `DescribeIdentityProvider` and `ListIdentityProviders` for the provider,
//! `GetIdentityProviderByIdentifier` for its identifiers, and
//! `DescribeUserPoolClient` for the app clients that referenced it.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The built-in provider an app client lists beside its third-party IdPs:
/// "The following are supported: COGNITO, Facebook, Google, SignInWithApple, and
/// LoginWithAmazon" (`CreateUserPoolClient.SupportedIdentityProviders`). It names
/// no stored IdP, so deleting a third-party IdP never removes it.
const BUILT_IN_PROVIDER: &str = "COGNITO";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// The sentence a lookup that resolved no IdP of the pool carries: it names what
/// the request asked for and the pool it was looked up in.
fn provider_not_found_message(asked_for: &str, user_pool_id: &str) -> String {
    format!("Identity provider {asked_for} for tenantId {user_pool_id} does not exist.")
}

/// The sentence a request naming a pool that does not exist carries.
fn pool_not_found_message(user_pool_id: &str) -> String {
    format!("User pool {user_pool_id} does not exist.")
}

/// Create an identity provider in `user_pool_id` with `identifiers`.
async fn create_provider(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    provider_name: &str,
    provider_type: &str,
    identifiers: &[&str],
) -> Value {
    ok(
        router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": provider_name,
            "ProviderType": provider_type,
            "ProviderDetails": details_for(provider_type),
            "IdpIdentifiers": identifiers,
        }),
    )
    .await
}

/// Detail maps that suit each provider type, so a row's type is the only thing
/// that varies. The keys are the ones
/// `smithy_query(query="doc",shape="CreateIdentityProviderRequest",member="ProviderDetails")`
/// documents for each IdP type; the values are opaque to the service.
fn details_for(provider_type: &str) -> Value {
    match provider_type {
        "SAML" => json!({ "MetadataFile": "<EntityDescriptor entityID=\"https://saml.example.com\"/>" }),
        "OIDC" => json!({
            "oidc_issuer": "https://auth.example.com",
            "client_id": "1example23456789",
            "authorize_scopes": "openid profile email",
        }),
        _ => json!({
            "client_id": "social-client-id",
            "client_secret": "social-client-secret",
            "authorize_scopes": "openid email",
        }),
    }
}

/// Create an app client of `user_pool_id`, with the `SupportedIdentityProviders`
/// the row asks for (`None` leaves the member OUT of the request), and return its
/// minted `ClientId`.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_name: &str,
    supported: Option<&[&str]>,
) -> String {
    let mut body = json!({ "UserPoolId": user_pool_id, "ClientName": client_name });
    if let Some(supported) = supported {
        body["SupportedIdentityProviders"] = json!(supported);
    }
    let created = ok(router, "CreateUserPoolClient", &body).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// The stored app client, as `DescribeUserPoolClient` renders it.
async fn read_client(router: &MultiServiceRouter, user_pool_id: &str, client_id: &str) -> Value {
    let read = ok(
        router,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    read["UserPoolClient"].clone()
}

/// The `ProviderName`s `ListIdentityProviders` reports for a pool.
async fn listed_provider_names(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListIdentityProviders",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    listed["Providers"]
        .as_array()
        .expect("Providers is a list")
        .iter()
        .map(|provider| {
            provider["ProviderName"]
                .as_str()
                .expect("a listed provider names itself")
                .to_string()
        })
        .collect()
}

/// One app client of a row: the name to create it under, the
/// `SupportedIdentityProviders` it is created with (`None` sends no member at
/// all) and the list it must hold once the row's provider is deleted.
struct ClientCase {
    client_name: &'static str,
    supported: Option<Vec<String>>,
    expected_after_delete: Option<Vec<String>>,
}

/// One row of the table: the provider to delete, a provider of the same pool that
/// must survive it, and the app clients that reference them.
struct DeleteCase {
    case: &'static str,
    /// The provider the row deletes, with its type and its IdP identifiers.
    target: (String, &'static str, Vec<String>),
    /// A second provider of the same pool, which the delete must not touch.
    survivor: (String, &'static str, Vec<String>),
    clients: Vec<ClientCase>,
}

/// Owned strings from borrowed ones, so a row can be written inline.
fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The table. The rows differ along what the rule generalises over: the deleted
/// provider's type (spanning the modelled `IdentityProviderTypeType` catalog from
/// `smithy_query(query="shape",shape="IdentityProviderTypeType")`), its name's
/// length and form, how many identifiers it carries, how many app clients
/// reference it, and WHERE in each client's list the reference sits — first, in
/// the middle, last, alone, or not at all.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "an OIDC IdP referenced in the middle of one client's list, beside a client \
                   that lists only the survivor and a client that lists no provider at all",
            target: (
                "AuthExampleOIDC".to_string(),
                "OIDC",
                names(&["auth.example.com", "auth.example.net"]),
            ),
            survivor: ("CorpSAML".to_string(), "SAML", names(&["corp.example.com"])),
            clients: vec![
                ClientCase {
                    client_name: "web",
                    supported: Some(names(&[
                        BUILT_IN_PROVIDER,
                        "AuthExampleOIDC",
                        "CorpSAML",
                    ])),
                    expected_after_delete: Some(names(&[BUILT_IN_PROVIDER, "CorpSAML"])),
                },
                ClientCase {
                    client_name: "saml only",
                    supported: Some(names(&["CorpSAML"])),
                    expected_after_delete: Some(names(&["CorpSAML"])),
                },
                ClientCase {
                    client_name: "no supported providers",
                    supported: None,
                    expected_after_delete: None,
                },
            ],
        },
        DeleteCase {
            case: "a SAML IdP carrying no identifiers, referenced FIRST by one client and as \
                   the ONLY provider of another",
            target: ("CorpSAML".to_string(), "SAML", Vec::new()),
            survivor: ("Google".to_string(), "Google", names(&["google.example"])),
            clients: vec![
                ClientCase {
                    client_name: "first in the list",
                    supported: Some(names(&["CorpSAML", BUILT_IN_PROVIDER, "Google"])),
                    expected_after_delete: Some(names(&[BUILT_IN_PROVIDER, "Google"])),
                },
                ClientCase {
                    client_name: "the only provider",
                    supported: Some(names(&["CorpSAML"])),
                    expected_after_delete: Some(Vec::new()),
                },
            ],
        },
        DeleteCase {
            case: "a Google IdP referenced LAST, by three clients at once",
            target: ("Google".to_string(), "Google", names(&["google.example"])),
            survivor: (
                "LoginWithAmazon".to_string(),
                "LoginWithAmazon",
                names(&["amazon.example"]),
            ),
            clients: vec![
                ClientCase {
                    client_name: "web",
                    supported: Some(names(&[BUILT_IN_PROVIDER, "Google"])),
                    expected_after_delete: Some(names(&[BUILT_IN_PROVIDER])),
                },
                ClientCase {
                    client_name: "mobile",
                    supported: Some(names(&["LoginWithAmazon", "Google"])),
                    expected_after_delete: Some(names(&["LoginWithAmazon"])),
                },
                ClientCase {
                    client_name: "machine",
                    supported: Some(names(&["Google"])),
                    expected_after_delete: Some(Vec::new()),
                },
            ],
        },
        DeleteCase {
            case: "a SignInWithApple IdP with the maximum 50 identifiers, referenced by a \
                   client that lists the survivor twice over the built-in provider",
            target: (
                "SignInWithApple".to_string(),
                "SignInWithApple",
                (0..50).map(|index| format!("apple-{index}")).collect(),
            ),
            survivor: ("Facebook".to_string(), "Facebook", Vec::new()),
            clients: vec![ClientCase {
                client_name: "hosted ui",
                supported: Some(names(&["Facebook", "SignInWithApple", BUILT_IN_PROVIDER])),
                expected_after_delete: Some(names(&["Facebook", BUILT_IN_PROVIDER])),
            }],
        },
        DeleteCase {
            case: "an IdP whose name is 32 characters, the modelled maximum, referenced by one \
                   client",
            target: ("n".repeat(32), "OIDC", names(&["n".repeat(40).as_str()])),
            survivor: ("abc".to_string(), "OIDC", names(&["a"])),
            clients: vec![ClientCase {
                client_name: "long name",
                supported: Some(vec![
                    BUILT_IN_PROVIDER.to_string(),
                    "n".repeat(32),
                    "abc".to_string(),
                ]),
                expected_after_delete: Some(names(&[BUILT_IN_PROVIDER, "abc"])),
            }],
        },
    ]
}

#[tokio::test]
async fn a_deleted_identity_provider_is_gone_and_every_app_client_reference_is_scrubbed() {
    for case in delete_cases() {
        let label = case.case;
        let router = router();
        let user_pool_id = pool(&router, "delete-idp-pool").await;
        let (target_name, target_type, target_identifiers) = &case.target;
        let (survivor_name, survivor_type, survivor_identifiers) = &case.survivor;

        create_provider(
            &router,
            &user_pool_id,
            target_name,
            target_type,
            &target_identifiers
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
        .await;
        let survivor_created = create_provider(
            &router,
            &user_pool_id,
            survivor_name,
            survivor_type,
            &survivor_identifiers
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
        .await;

        let mut client_ids = Vec::new();
        for client in &case.clients {
            let supported: Option<Vec<&str>> = client
                .supported
                .as_ref()
                .map(|names| names.iter().map(String::as_str).collect());
            let client_id = create_client(
                &router,
                &user_pool_id,
                client.client_name,
                supported.as_deref(),
            )
            .await;
            client_ids.push(client_id);
        }
        let before: Vec<Value> = {
            let mut before = Vec::new();
            for client_id in &client_ids {
                before.push(read_client(&router, &user_pool_id, client_id).await);
            }
            before
        };

        let (status, payload) = call(
            &router,
            "DeleteIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": target_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting a stored IdP succeeds, got {status} with {payload}"
        );

        // The provider is gone from every reader of the same state.
        let (status, payload) = call(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": target_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a deleted IdP is no longer readable, got {payload}"
        );
        let listed = listed_provider_names(&router, &user_pool_id).await;
        assert_eq!(
            listed,
            vec![survivor_name.clone()],
            "case {label:?}: only the surviving IdP is listed, got {listed:?}"
        );

        // The identifiers it carried went with it, so a lookup by identifier no
        // longer resolves it.
        for identifier in target_identifiers {
            let (status, payload) = call(
                &router,
                "GetIdentityProviderByIdentifier",
                &json!({ "UserPoolId": user_pool_id, "IdpIdentifier": identifier }),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (404, "ResourceNotFoundException"),
                "case {label:?}: identifier {identifier:?} of a deleted IdP resolves nothing, got {payload}"
            );
        }

        // The other provider of the same pool is untouched, member for member.
        let survivor_read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": survivor_name }),
        )
        .await;
        assert_eq!(
            survivor_read["IdentityProvider"], survivor_created["IdentityProvider"],
            "case {label:?}: the surviving IdP is exactly as it was created, got {survivor_read}"
        );

        // Every app client that listed the deleted provider no longer lists it,
        // keeps its other entries in order, and is otherwise unchanged.
        for (client, (client_id, before)) in case
            .clients
            .iter()
            .zip(client_ids.iter().zip(before.iter()))
        {
            let after = read_client(&router, &user_pool_id, client_id).await;
            let expected = client
                .expected_after_delete
                .as_ref()
                .map(|names| json!(names));
            assert_eq!(
                after.get("SupportedIdentityProviders").cloned(),
                expected,
                "case {label:?}: client {:?} must list exactly the providers that survive, got {after}",
                client.client_name
            );

            let mut expected_client = before.clone();
            match &expected {
                Some(providers) => {
                    expected_client["SupportedIdentityProviders"] = providers.clone();
                }
                None => {
                    expected_client
                        .as_object_mut()
                        .expect("an app client is a JSON object")
                        .remove("SupportedIdentityProviders");
                }
            }
            assert_eq!(
                after, expected_client,
                "case {label:?}: client {:?} must be unchanged apart from the scrubbed reference, got {after}",
                client.client_name
            );
        }
    }
}

#[tokio::test]
async fn a_delete_that_resolves_no_provider_or_no_pool_is_a_404_and_persists_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "absent-idp-delete-pool").await;
    let created = create_provider(
        &router,
        &user_pool_id,
        "AuthExampleOIDC",
        "OIDC",
        &["auth.example.com"],
    )
    .await;
    let client_id = create_client(
        &router,
        &user_pool_id,
        "web",
        Some(&[BUILT_IN_PROVIDER, "AuthExampleOIDC"]),
    )
    .await;
    let client_before = read_client(&router, &user_pool_id, &client_id).await;

    // Every way a delete can resolve nothing: a provider name the pool never
    // held, one differing only in case from a name it does hold (ProviderNameType
    // carries no case-insensitivity trait), a name only another pool holds, and a
    // pool that does not exist.
    let other_pool = pool(&router, "other-idp-delete-pool").await;
    create_provider(&router, &other_pool, "OtherOnly", "SAML", &[]).await;
    for (label, provider_name, target_pool, expected_message) in [
        (
            "a provider name the pool never held",
            "AbsentProvider".to_string(),
            user_pool_id.clone(),
            provider_not_found_message("AbsentProvider", &user_pool_id),
        ),
        (
            "a name differing only in case from a stored one",
            "AUTHEXAMPLEOIDC".to_string(),
            user_pool_id.clone(),
            provider_not_found_message("AUTHEXAMPLEOIDC", &user_pool_id),
        ),
        (
            "a name only another pool holds",
            "OtherOnly".to_string(),
            user_pool_id.clone(),
            provider_not_found_message("OtherOnly", &user_pool_id),
        ),
        (
            "a user pool that does not exist",
            "AuthExampleOIDC".to_string(),
            "us-east-1_absent".to_string(),
            pool_not_found_message("us-east-1_absent"),
        ),
    ] {
        let (status, payload) = call(
            &router,
            "DeleteIdentityProvider",
            &json!({ "UserPoolId": target_pool, "ProviderName": provider_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: must be a 404 ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            payload["message"],
            json!(expected_message),
            "case {label:?}: the refusal names what was asked for and where, got {payload}"
        );
    }

    // None of the refused deletes touched the stored provider, the other pool's
    // provider, or the app client that references the stored one.
    let read = ok(
        &router,
        "DescribeIdentityProvider",
        &json!({ "UserPoolId": user_pool_id, "ProviderName": "AuthExampleOIDC" }),
    )
    .await;
    assert_eq!(
        read["IdentityProvider"], created["IdentityProvider"],
        "a refused delete leaves the stored IdP exactly as it was, got {read}"
    );
    let other_listed = listed_provider_names(&router, &other_pool).await;
    assert_eq!(
        other_listed,
        vec!["OtherOnly".to_string()],
        "a refused delete leaves another pool's IdP alone, got {other_listed:?}"
    );
    let client_after = read_client(&router, &user_pool_id, &client_id).await;
    assert_eq!(
        client_after, client_before,
        "a refused delete scrubs no app client reference, got {client_after}"
    );

    // A second delete of the SAME provider resolves nothing, and says so in the
    // same sentence.
    ok(
        &router,
        "DeleteIdentityProvider",
        &json!({ "UserPoolId": user_pool_id, "ProviderName": "AuthExampleOIDC" }),
    )
    .await;
    let (status, payload) = call(
        &router,
        "DeleteIdentityProvider",
        &json!({ "UserPoolId": user_pool_id, "ProviderName": "AuthExampleOIDC" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (404, "ResourceNotFoundException"),
        "a second delete of the same IdP is a 404 ResourceNotFoundException, got {payload}"
    );
    assert_eq!(
        payload["message"],
        json!(provider_not_found_message("AuthExampleOIDC", &user_pool_id)),
        "the second delete is refused in the same sentence as any other absent IdP, got {payload}"
    );
}

#[tokio::test]
async fn a_delete_is_scoped_to_the_pool_it_names() {
    let router = router();
    let first_pool = pool(&router, "first-delete-idp-pool").await;
    let second_pool = pool(&router, "second-delete-idp-pool").await;

    // The same provider name in two pools is two providers, each with its own
    // referencing app client.
    let mut client_ids = Vec::new();
    for user_pool_id in [&first_pool, &second_pool] {
        create_provider(&router, user_pool_id, "SharedName", "OIDC", &["shared"]).await;
        client_ids.push(
            create_client(
                &router,
                user_pool_id,
                "web",
                Some(&[BUILT_IN_PROVIDER, "SharedName"]),
            )
            .await,
        );
    }

    ok(
        &router,
        "DeleteIdentityProvider",
        &json!({ "UserPoolId": first_pool, "ProviderName": "SharedName" }),
    )
    .await;

    let first_listed = listed_provider_names(&router, &first_pool).await;
    assert!(
        first_listed.is_empty(),
        "the named pool no longer holds the deleted IdP, got {first_listed:?}"
    );
    let second_listed = listed_provider_names(&router, &second_pool).await;
    assert_eq!(
        second_listed,
        vec!["SharedName".to_string()],
        "the other pool's IdP of the same name survives, got {second_listed:?}"
    );

    // The app client of the pool the delete named lost its reference; the one in
    // the other pool kept it, so the scrub is scoped to that pool.
    let first_client = read_client(&router, &first_pool, &client_ids[0]).await;
    assert_eq!(
        first_client["SupportedIdentityProviders"],
        json!([BUILT_IN_PROVIDER]),
        "the named pool's app client lost the reference, got {first_client}"
    );
    let second_client = read_client(&router, &second_pool, &client_ids[1]).await;
    assert_eq!(
        second_client["SupportedIdentityProviders"],
        json!([BUILT_IN_PROVIDER, "SharedName"]),
        "an app client of another pool keeps its reference, got {second_client}"
    );
}

#[tokio::test]
async fn a_delete_outside_a_modelled_constraint_is_refused_as_a_bad_request() {
    let router = router();
    let user_pool_id = pool(&router, "delete-idp-constraint-pool").await;
    create_provider(&router, &user_pool_id, "AuthExampleOIDC", "OIDC", &[]).await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="DeleteIdentityProvider")`): the
    // provider name's length 1..32, and the user pool id's pattern.
    for (label, body) in [
        (
            "an empty provider name, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "ProviderName": "" }),
        ),
        (
            "a provider name of 33 characters, past the length maximum 32",
            json!({ "UserPoolId": user_pool_id, "ProviderName": "p".repeat(33) }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "ProviderName": "AuthExampleOIDC" }),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteIdentityProvider", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // And none of them deleted anything.
    let listed = listed_provider_names(&router, &user_pool_id).await;
    assert_eq!(
        listed,
        vec!["AuthExampleOIDC".to_string()],
        "a refused request deletes no IdP, got {listed:?}"
    );
}
