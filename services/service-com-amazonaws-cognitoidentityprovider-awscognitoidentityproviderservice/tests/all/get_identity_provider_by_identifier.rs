// SPDX-License-Identifier: CC0-1.0

//! `GetIdentityProviderByIdentifier` over the wire, driven through the router
//! exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: the operation resolves an IdP identifier against the
//! identifier list STORED on the providers of the user pool the request names,
//! matching an entry exactly, and renders that stored provider — the same
//! document `DescribeIdentityProvider` renders for it; an identifier no provider
//! of that pool carries, including one carried only by a provider of another
//! pool, is a 400 `ResourceNotFoundException` whose message is built from the
//! request (it names the identifier asked for and the pool it was looked up in),
//! and an absent pool is a 400 reported as the unknown pool. Each table row
//! names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
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

/// A JSON object from key/value pairs, in the order given.
fn object(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// One row: a provider to create in the pool, and which of its stored
/// identifiers the lookup is driven with. The rows differ along what the rule
/// generalises over — the provider type, the number of identifiers the provider
/// carries, which position in that list is looked up, and the identifier's
/// length and character class.
struct Case {
    case: &'static str,
    provider_name: String,
    provider_type: &'static str,
    details: Vec<(&'static str, &'static str)>,
    identifiers: Vec<String>,
    /// The index into `identifiers` the lookup uses.
    looked_up: usize,
}

/// The table. `ProviderType` spans the modelled catalog —
/// `smithy_query(query="shape",shape="IdentityProviderTypeType")`: SAML,
/// Facebook, Google, LoginWithAmazon, SignInWithApple, OIDC — and the rows span
/// `IdpIdentifiers`' list length 1..50 with member length 1..40 and pattern
/// `^[\w\s+=.@-]+$`, every constraint from
/// `smithy_query(query="operation",operation="GetIdentityProviderByIdentifier")`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "an OIDC IdP whose single identifier is a domain name",
            provider_name: "AuthExampleOIDC".to_string(),
            provider_type: "OIDC",
            details: vec![
                ("oidc_issuer", "https://auth.example.com"),
                ("client_id", "1example23456789"),
            ],
            identifiers: vec!["auth.example.com".to_string()],
            looked_up: 0,
        },
        Case {
            case: "a SAML IdP with three identifiers, looked up by the LAST one",
            provider_name: "CorpSAML".to_string(),
            provider_type: "SAML",
            details: vec![(
                "MetadataFile",
                "<EntityDescriptor entityID=\"https://saml.example.com\"/>",
            )],
            identifiers: vec![
                "corp".to_string(),
                "corp.example.com".to_string(),
                "corp+eu@example.com".to_string(),
            ],
            looked_up: 2,
        },
        Case {
            case: "a Google IdP with the identifier list at its maximum 50, looked up in the middle",
            provider_name: "Google".to_string(),
            provider_type: "Google",
            details: vec![("client_id", "google-client-id")],
            identifiers: (0..50).map(|index| format!("google-{index}")).collect(),
            looked_up: 25,
        },
        Case {
            case: "a Facebook IdP whose identifier is one character, the length minimum",
            provider_name: "Facebook".to_string(),
            provider_type: "Facebook",
            details: vec![("client_id", "facebook-client-id")],
            identifiers: vec!["f".to_string()],
            looked_up: 0,
        },
        Case {
            case: "a LoginWithAmazon IdP whose identifier is 40 characters, the length maximum",
            provider_name: "LoginWithAmazon".to_string(),
            provider_type: "LoginWithAmazon",
            details: vec![("client_id", "amzn1.application-oa2-client.example")],
            identifiers: vec!["a".repeat(40)],
            looked_up: 0,
        },
        Case {
            case: "a SignInWithApple IdP with a 32-character name and a spaced identifier",
            provider_name: "s".repeat(32),
            provider_type: "SignInWithApple",
            details: vec![("client_id", "com.example.service")],
            identifiers: vec!["Example Corp".to_string(), "example=corp".to_string()],
            looked_up: 0,
        },
    ]
}

impl Case {
    /// This case's `CreateIdentityProvider` request for `user_pool_id`.
    fn request(&self, user_pool_id: &str) -> Value {
        json!({
            "UserPoolId": user_pool_id,
            "ProviderName": self.provider_name,
            "ProviderType": self.provider_type,
            "ProviderDetails": object(&self.details),
            "IdpIdentifiers": self.identifiers,
        })
    }

    /// The identifier the lookup is driven with.
    fn identifier(&self) -> &str {
        &self.identifiers[self.looked_up]
    }
}

#[tokio::test]
async fn an_identifier_resolves_to_the_stored_provider_that_carries_it() {
    let router = router();
    let user_pool_id = pool(&router, "get-idp-by-identifier-pool").await;

    // Every row's provider lives in ONE pool at the same time, so each lookup
    // has to pick the provider carrying the identifier out of all of them.
    let cases = cases();
    for case in &cases {
        ok(&router, "CreateIdentityProvider", &case.request(&user_pool_id)).await;
    }

    for case in &cases {
        let label = case.case;
        let found = ok(
            &router,
            "GetIdentityProviderByIdentifier",
            &json!({ "UserPoolId": user_pool_id, "IdpIdentifier": case.identifier() }),
        )
        .await;
        let provider = &found["IdentityProvider"];

        assert_eq!(
            provider["ProviderName"],
            json!(case.provider_name),
            "case {label:?}: the identifier must resolve to the provider carrying it, got {provider}"
        );
        assert_eq!(
            provider["ProviderType"],
            json!(case.provider_type),
            "case {label:?}: the resolved provider renders its stored type, got {provider}"
        );
        assert_eq!(
            provider["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the resolved provider names the pool it was created in, got {provider}"
        );
        assert_eq!(
            provider["ProviderDetails"],
            super::identity_provider_type_defaults::resolved_details(
                case.provider_type,
                &object(&case.details)
            ),
            "case {label:?}: the resolved provider renders its stored details, got {provider}"
        );
        assert_eq!(
            provider["IdpIdentifiers"],
            json!(case.identifiers),
            "case {label:?}: the resolved provider renders its stored identifiers, in order, got {provider}"
        );

        // The document is exactly what the by-name reader renders for the same
        // provider: one stored resource, two ways in.
        let described = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": case.provider_name }),
        )
        .await;
        assert_eq!(
            provider, &described["IdentityProvider"],
            "case {label:?}: the by-identifier read must render the same stored provider as the by-name read, got {described}"
        );

        // And the read changes nothing a later caller sees.
        let again = ok(
            &router,
            "GetIdentityProviderByIdentifier",
            &json!({ "UserPoolId": user_pool_id, "IdpIdentifier": case.identifier() }),
        )
        .await;
        assert_eq!(
            again["IdentityProvider"], found["IdentityProvider"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }

    // Every OTHER stored identifier of each provider resolves to that same
    // provider too: the lookup is over the whole list, not just one entry.
    for case in &cases {
        let label = case.case;
        for identifier in &case.identifiers {
            let found = ok(
                &router,
                "GetIdentityProviderByIdentifier",
                &json!({ "UserPoolId": user_pool_id, "IdpIdentifier": identifier }),
            )
            .await;
            assert_eq!(
                found["IdentityProvider"]["ProviderName"],
                json!(case.provider_name),
                "case {label:?}: identifier {identifier:?} must resolve to its provider, got {found}"
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_is_scoped_to_the_pool_the_request_names() {
    let router = router();
    let first_pool = pool(&router, "first-get-idp-pool").await;
    let second_pool = pool(&router, "second-get-idp-pool").await;

    // The SAME identifier in two pools is two providers; each pool resolves its
    // own, and a pool holding no provider with that identifier is a 400.
    for (label, user_pool_id, provider_name, issuer) in [
        (
            "the first pool",
            &first_pool,
            "FirstOIDC",
            "https://first.example.com",
        ),
        (
            "the second pool",
            &second_pool,
            "SecondOIDC",
            "https://second.example.com",
        ),
    ] {
        ok(
            &router,
            "CreateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": provider_name,
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": issuer },
                "IdpIdentifiers": ["shared.example.com"],
            }),
        )
        .await;
        let found = ok(
            &router,
            "GetIdentityProviderByIdentifier",
            &json!({ "UserPoolId": user_pool_id, "IdpIdentifier": "shared.example.com" }),
        )
        .await;
        assert_eq!(
            found["IdentityProvider"]["ProviderName"],
            json!(provider_name),
            "case {label:?}: each pool resolves its OWN provider for the shared identifier, got {found}"
        );
    }

    // An identifier only the OTHER pool's provider carries is not found here.
    ok(
        &router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": first_pool,
            "ProviderName": "FirstOnly",
            "ProviderType": "SAML",
            "ProviderDetails": { "MetadataFile": "<EntityDescriptor/>" },
            "IdpIdentifiers": ["first-only.example.com"],
        }),
    )
    .await;
    let (status, payload) = call(
        &router,
        "GetIdentityProviderByIdentifier",
        &json!({ "UserPoolId": second_pool, "IdpIdentifier": "first-only.example.com" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "an identifier carried only by another pool's provider is a 400 ResourceNotFoundException, got {payload}"
    );
    assert_eq!(
        payload["message"],
        json!(identifier_not_found_message(
            "first-only.example.com",
            &second_pool
        )),
        "the refusal names the identifier asked for and the pool it was looked up in, got {payload}"
    );
}

/// The message a lookup that resolved no provider carries: built from the
/// request, naming the identifier asked for and the pool it was looked up in.
fn identifier_not_found_message(idp_identifier: &str, user_pool_id: &str) -> String {
    format!("Identity provider {idp_identifier} for tenantId {user_pool_id} does not exist.")
}

#[tokio::test]
async fn an_identifier_no_provider_of_the_pool_carries_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "absent-identifier-pool").await;
    ok(
        &router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": "AuthExampleOIDC",
            "ProviderType": "OIDC",
            "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
            "IdpIdentifiers": ["auth.example.com"],
        }),
    )
    .await;
    // A provider created with NO identifiers is reachable by name only.
    ok(
        &router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": "NoIdentifiers",
            "ProviderType": "SAML",
            "ProviderDetails": { "MetadataFile": "<EntityDescriptor/>" },
        }),
    )
    .await;

    for (label, identifier) in [
        (
            "an identifier nothing in the pool carries",
            "absent.example.com".to_string(),
        ),
        (
            "the provider's NAME, which is not one of its identifiers",
            "AuthExampleOIDC".to_string(),
        ),
        (
            "an identifier differing from a stored one only in case",
            "AUTH.EXAMPLE.COM".to_string(),
        ),
        (
            "a prefix of a stored identifier, which is not an exact match",
            "auth.example".to_string(),
        ),
        (
            "the name of a provider created with no identifiers",
            "NoIdentifiers".to_string(),
        ),
    ] {
        let (status, payload) = call(
            &router,
            "GetIdentityProviderByIdentifier",
            &json!({ "UserPoolId": user_pool_id, "IdpIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
        // The message is built from the request, so it names the identifier the
        // caller asked with and the pool it was looked up in.
        assert_eq!(
            payload["message"],
            json!(identifier_not_found_message(&identifier, &user_pool_id)),
            "case {label:?}: the refusal names the identifier and the pool it was looked up in, got {payload}"
        );
    }

    // An absent POOL is the same status and code, reported as the unknown pool
    // rather than as an unknown identifier within it.
    let (status, payload) = call(
        &router,
        "GetIdentityProviderByIdentifier",
        &json!({ "UserPoolId": "us-east-1_absent", "IdpIdentifier": "auth.example.com" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "an absent user pool is a 400 ResourceNotFoundException, got {payload}"
    );
    assert_eq!(
        payload["message"],
        json!("User pool us-east-1_absent does not exist."),
        "an absent pool is reported as the unknown pool, got {payload}"
    );

    // The failed lookups left the stored provider exactly as it was.
    let described = ok(
        &router,
        "DescribeIdentityProvider",
        &json!({ "UserPoolId": user_pool_id, "ProviderName": "AuthExampleOIDC" }),
    )
    .await;
    assert_eq!(
        described["IdentityProvider"]["IdpIdentifiers"],
        json!(["auth.example.com"]),
        "a failed lookup must not touch the stored provider, got {described}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "get-idp-constraint-pool").await;

    // One value past each published bound of the input
    // (`smithy_query(query="operation",operation="GetIdentityProviderByIdentifier")`):
    // `IdpIdentifier`'s length 1..40 and pattern `^[\w\s+=.@-]+$`, and
    // `UserPoolId`'s pattern `^[\w-]+_[0-9a-zA-Z]+$`.
    for (label, body) in [
        (
            "an empty identifier, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "IdpIdentifier": "" }),
        ),
        (
            "an identifier of 41 characters, past the length maximum 40",
            json!({ "UserPoolId": user_pool_id, "IdpIdentifier": "i".repeat(41) }),
        ),
        (
            "an identifier holding a slash, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "IdpIdentifier": "auth/example" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "IdpIdentifier": "auth.example.com" }),
        ),
    ] {
        let (status, payload) = call(&router, "GetIdentityProviderByIdentifier", &body).await;
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
}
