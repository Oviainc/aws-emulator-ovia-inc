// SPDX-License-Identifier: CC0-1.0

//! Third-party identity providers over the wire: `CreateIdentityProvider` stores
//! an IdP configuration in the pool it names and every later
//! `DescribeIdentityProvider` renders the STORED provider, driven through the
//! router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a created identity provider is keyed by (`UserPoolId`,
//! `ProviderName`), it renders `UserPoolId`, `ProviderName`, `ProviderType`,
//! `ProviderDetails`, `AttributeMapping` and `IdpIdentifiers` as submitted (empty
//! when the request set none), completed by the configuration the submitted
//! `ProviderType` resolves ([`super::identity_provider_type_defaults`]), beside
//! the `AcrMapping` only an IdP created with one carries and the instant it was
//! created, a second provider under a name the pool already holds is refused and
//! leaves the stored one untouched, and an absent pool or provider name is a 400.
//! Each table row names the case it asserts.

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
fn object(entries: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.clone(), json!(value));
    }
    Value::Object(map)
}

/// One row of the `CreateIdentityProvider` table: the provider's name and type,
/// the detail map, and the optional members the request may leave out. The rows
/// differ along what the rule generalises over — the provider type, the size and
/// content of each map, and the count and length of the identifiers.
struct ProviderCase {
    case: &'static str,
    provider_name: String,
    provider_type: &'static str,
    details: Vec<(String, String)>,
    /// `None` leaves the member OUT of the request; `Some(..)` sends exactly it.
    attribute_mapping: Option<Vec<(String, String)>>,
    idp_identifiers: Option<Vec<String>>,
    acr_mapping: Option<Vec<(String, String)>>,
}

impl ProviderCase {
    /// This case's `CreateIdentityProvider` request for `user_pool_id`: a member
    /// the case leaves unset is left OUT of the request, not sent as null.
    fn request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ProviderName".to_string(), json!(self.provider_name));
        body.insert("ProviderType".to_string(), json!(self.provider_type));
        body.insert("ProviderDetails".to_string(), object(&self.details));
        if let Some(mapping) = &self.attribute_mapping {
            body.insert("AttributeMapping".to_string(), object(mapping));
        }
        if let Some(identifiers) = &self.idp_identifiers {
            body.insert("IdpIdentifiers".to_string(), json!(identifiers));
        }
        if let Some(mapping) = &self.acr_mapping {
            body.insert("AcrMapping".to_string(), object(mapping));
        }
        Value::Object(body)
    }

    /// What the stored provider must render for `AttributeMapping`: the submitted
    /// mapping (an empty one when the request set none), completed by the mapping
    /// of the submitted type's own subject claim.
    fn expected_attribute_mapping(&self) -> Value {
        let submitted = match &self.attribute_mapping {
            Some(mapping) => object(mapping),
            None => json!({}),
        };
        super::identity_provider_type_defaults::resolved_attribute_mapping(
            self.provider_type,
            &submitted,
        )
    }

    /// Likewise `ProviderDetails`: the submitted map, completed by the entries
    /// the submitted type resolves.
    fn expected_details(&self) -> Value {
        super::identity_provider_type_defaults::resolved_details(
            self.provider_type,
            &object(&self.details),
        )
    }

    /// What the stored provider must render for `IdpIdentifiers`: the submitted
    /// list, in order, and an empty list when the request set none.
    fn expected_idp_identifiers(&self) -> Value {
        match &self.idp_identifiers {
            Some(identifiers) => json!(identifiers),
            None => json!([]),
        }
    }

    /// What the stored provider must render for `AcrMapping`: the submitted map,
    /// and nothing at all when the request set none — an absent ACR mapping is
    /// not invented as an empty one.
    fn expected_acr_mapping(&self) -> Option<Value> {
        self.acr_mapping.as_ref().map(|mapping| object(mapping))
    }
}

/// Owned key/value pairs from borrowed ones, so a row can be written inline.
fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The table. `ProviderType` spans the whole modelled catalog — SAML, Facebook,
/// Google, LoginWithAmazon, SignInWithApple, OIDC, from
/// `smithy_query(query="shape",shape="IdentityProviderTypeType")` — and the rows
/// span `ProviderName`'s length 1..32, `ProviderDetails`' arbitrary keys and
/// value length 0..131072, `IdpIdentifiers`' list length 0..50 with member
/// length 1..40, and `AcrMapping`'s length 0..4 with keys `Level1`..`Level4`,
/// every constraint from
/// `smithy_query(query="operation",operation="CreateIdentityProvider")`.
fn provider_cases() -> Vec<ProviderCase> {
    vec![
        ProviderCase {
            case: "an OIDC IdP with discovery details, an attribute mapping, one identifier \
                   and all four ACR levels",
            provider_name: "AuthExampleOIDC".to_string(),
            provider_type: "OIDC",
            details: pairs(&[
                ("attributes_request_method", "GET"),
                ("attributes_url", "https://auth.example.com/userInfo"),
                ("authorize_scopes", "openid profile email"),
                ("authorize_url", "https://auth.example.com/authorize"),
                ("client_id", "1example23456789"),
                ("client_secret", "provider-app-client-secret"),
                ("jwks_uri", "https://auth.example.com/.well-known/jwks.json"),
                ("oidc_issuer", "https://auth.example.com"),
                ("token_url", "https://auth.example.com/token"),
            ]),
            attribute_mapping: Some(pairs(&[("email", "email"), ("username", "sub")])),
            idp_identifiers: Some(vec!["auth.example.com".to_string()]),
            acr_mapping: Some(pairs(&[
                ("Level1", "urn:acr:1"),
                ("Level2", "urn:acr:2"),
                ("Level3", "urn:acr:3"),
                ("Level4", "urn:acr:4"),
            ])),
        },
        ProviderCase {
            case: "a SAML IdP from a metadata document, with every optional member left out",
            provider_name: "CorpSAML".to_string(),
            provider_type: "SAML",
            details: pairs(&[
                (
                    "MetadataFile",
                    "<EntityDescriptor entityID=\"https://saml.example.com\"/>",
                ),
                ("IDPSignout", "true"),
            ]),
            attribute_mapping: None,
            idp_identifiers: None,
            acr_mapping: None,
        },
        ProviderCase {
            case: "a Google IdP with the identifier list at its maximum 50",
            provider_name: "Google".to_string(),
            provider_type: "Google",
            details: pairs(&[
                ("client_id", "google-client-id"),
                ("client_secret", "google-client-secret"),
                ("authorize_scopes", "openid email profile"),
            ]),
            attribute_mapping: Some(pairs(&[
                ("email", "email"),
                ("given_name", "given_name"),
                ("username", "sub"),
            ])),
            idp_identifiers: Some((0..50).map(|index| format!("google-{index}")).collect()),
            acr_mapping: Some(pairs(&[("Level1", "low")])),
        },
        ProviderCase {
            case: "a Facebook IdP whose ProviderDetails map is present but empty",
            provider_name: "Facebook".to_string(),
            provider_type: "Facebook",
            details: Vec::new(),
            attribute_mapping: Some(Vec::new()),
            idp_identifiers: Some(Vec::new()),
            acr_mapping: Some(Vec::new()),
        },
        ProviderCase {
            case: "a LoginWithAmazon IdP with identifiers at both length bounds, 1 and 40",
            provider_name: "LoginWithAmazon".to_string(),
            provider_type: "LoginWithAmazon",
            details: pairs(&[
                ("client_id", "amzn1.application-oa2-client.example"),
                ("client_secret", "amazon-client-secret"),
                ("authorize_scopes", "profile postal_code"),
            ]),
            attribute_mapping: None,
            idp_identifiers: Some(vec!["a".to_string(), "i".repeat(40)]),
            acr_mapping: None,
        },
        ProviderCase {
            case: "a SignInWithApple IdP with an empty detail value and a 32-character name",
            provider_name: "s".repeat(32),
            provider_type: "SignInWithApple",
            details: pairs(&[
                ("client_id", "com.example.service"),
                ("team_id", "1EXAMPLE23"),
                ("key_id", "2EXAMPLE34"),
                ("private_key", ""),
                ("authorize_scopes", "name email"),
            ]),
            attribute_mapping: Some(pairs(&[("email", "email")])),
            idp_identifiers: None,
            acr_mapping: None,
        },
    ]
}

#[tokio::test]
async fn a_created_identity_provider_round_trips_through_stored_state() {
    let router = router();
    let user_pool_id = pool(&router, "identity-provider-pool").await;

    for case in provider_cases() {
        let label = case.case;
        let created = ok(&router, "CreateIdentityProvider", &case.request(&user_pool_id)).await;
        let provider = &created["IdentityProvider"];

        assert_eq!(
            provider["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the provider must name the pool it was created in, got {provider}"
        );
        assert_eq!(
            provider["ProviderName"],
            json!(case.provider_name),
            "case {label:?}: the stored provider name must be the created one, got {provider}"
        );
        assert_eq!(
            provider["ProviderType"],
            json!(case.provider_type),
            "case {label:?}: the stored provider type must be the created one, got {provider}"
        );
        assert_eq!(
            provider["ProviderDetails"],
            case.expected_details(),
            "case {label:?}: the stored details must be the submitted ones completed by the type's own, got {provider}"
        );
        assert_eq!(
            provider["AttributeMapping"],
            case.expected_attribute_mapping(),
            "case {label:?}: the stored attribute mapping must be the submitted one completed by the type's subject claim, got {provider}"
        );
        assert_eq!(
            provider["IdpIdentifiers"],
            case.expected_idp_identifiers(),
            "case {label:?}: the stored identifiers must be the submitted ones, in order, got {provider}"
        );
        assert_eq!(
            provider.get("AcrMapping").cloned(),
            case.expected_acr_mapping(),
            "case {label:?}: ACR mapping is rendered only for a provider created with one, got {provider}"
        );

        // A created provider carries the instant it was created, in both date
        // members, as a number of seconds (`DateType`).
        let creation_date = provider["CreationDate"].as_f64();
        assert!(
            creation_date.is_some_and(|seconds| seconds > 0.0),
            "case {label:?}: a created provider carries its creation instant, got {provider}"
        );
        assert_eq!(
            provider["LastModifiedDate"], provider["CreationDate"],
            "case {label:?}: a provider nothing has updated was last modified when it was created, got {provider}"
        );

        // The round trip: the reader renders the STORED provider, member for
        // member.
        let read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": case.provider_name }),
        )
        .await;
        assert_eq!(
            read["IdentityProvider"], created["IdentityProvider"],
            "case {label:?}: DescribeIdentityProvider must render the stored provider, got {read}"
        );

        // And reading it again changes nothing a later caller sees.
        let again = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": case.provider_name }),
        )
        .await;
        assert_eq!(
            again["IdentityProvider"], read["IdentityProvider"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn a_provider_name_the_pool_already_holds_is_refused_and_the_stored_provider_survives() {
    let router = router();
    let user_pool_id = pool(&router, "duplicate-provider-pool").await;

    for case in provider_cases() {
        let label = case.case;
        let created = ok(&router, "CreateIdentityProvider", &case.request(&user_pool_id)).await;

        // A second provider under the same name, with a DIFFERENT type and
        // details: refused, and the stored one is left exactly as it was.
        let (status, payload) = call(
            &router,
            "CreateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": case.provider_name,
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://second.example.com" },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "DuplicateProviderException"),
            "case {label:?}: a provider name already supported by the pool is a 400 DuplicateProviderException, got {payload}"
        );

        let read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": case.provider_name }),
        )
        .await;
        assert_eq!(
            read["IdentityProvider"], created["IdentityProvider"],
            "case {label:?}: a refused create must not have touched the stored provider, got {read}"
        );
    }
}

#[tokio::test]
async fn a_provider_is_keyed_by_its_pool_and_its_exact_provider_name() {
    let router = router();
    let first_pool = pool(&router, "first-idp-pool").await;
    let second_pool = pool(&router, "second-idp-pool").await;

    // The same provider name in two pools is two providers, each naming its own
    // pool and keeping its own details.
    for (label, user_pool_id, issuer) in [
        ("the first pool", &first_pool, "https://first.example.com"),
        ("the second pool", &second_pool, "https://second.example.com"),
    ] {
        let created = ok(
            &router,
            "CreateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": "SharedName",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": issuer },
            }),
        )
        .await;
        assert_eq!(
            created["IdentityProvider"]["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the provider must name the pool it was created in, got {created}"
        );
    }
    for (label, user_pool_id, issuer) in [
        ("the first pool", &first_pool, "https://first.example.com"),
        ("the second pool", &second_pool, "https://second.example.com"),
    ] {
        let read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": "SharedName" }),
        )
        .await;
        assert_eq!(
            read["IdentityProvider"]["ProviderDetails"]["oidc_issuer"],
            json!(issuer),
            "case {label:?}: each pool's provider keeps its own details, got {read}"
        );
    }

    // `ProviderNameType` carries no case-insensitivity trait, so names differing
    // only in case are different providers.
    let created = ok(
        &router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": first_pool,
            "ProviderName": "SHAREDNAME",
            "ProviderType": "SAML",
            "ProviderDetails": { "MetadataFile": "<EntityDescriptor/>" },
        }),
    )
    .await;
    assert_eq!(
        created["IdentityProvider"]["ProviderName"], "SHAREDNAME",
        "a name differing only in case is a different provider, got {created}"
    );
    let read = ok(
        &router,
        "DescribeIdentityProvider",
        &json!({ "UserPoolId": first_pool, "ProviderName": "SharedName" }),
    )
    .await;
    assert_eq!(
        read["IdentityProvider"]["ProviderType"],
        json!("OIDC"),
        "a provider is matched by its exact name, got {read}"
    );
}

#[tokio::test]
async fn an_absent_pool_or_provider_name_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "notfound-idp-pool").await;
    ok(
        &router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": "AuthExampleOIDC",
            "ProviderType": "OIDC",
            "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
        }),
    )
    .await;

    for (label, operation, body) in [
        (
            "CreateIdentityProvider on an absent pool",
            "CreateIdentityProvider",
            json!({
                "UserPoolId": "us-east-1_absent",
                "ProviderName": "AuthExampleOIDC",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
            }),
        ),
        (
            "DescribeIdentityProvider on an absent pool",
            "DescribeIdentityProvider",
            json!({ "UserPoolId": "us-east-1_absent", "ProviderName": "AuthExampleOIDC" }),
        ),
        (
            "DescribeIdentityProvider for a provider name the pool does not hold",
            "DescribeIdentityProvider",
            json!({ "UserPoolId": user_pool_id, "ProviderName": "AbsentProvider" }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "idp-constraint-pool").await;

    // One value past each published bound of `CreateIdentityProvider`'s input
    // (`smithy_query(query="operation",operation="CreateIdentityProvider")`): the
    // provider name's length 1..32 and pattern, the identifier list's length
    // 0..50 with member length 1..40, and the ACR map's length 0..4 with keys
    // `Level1`..`Level4`.
    let over_long_identifiers: Vec<String> = (0..51).map(|index| format!("idp-{index}")).collect();
    for (label, body) in [
        (
            "an empty provider name, below the length minimum 1",
            json!({
                "UserPoolId": user_pool_id,
                "ProviderName": "",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
            }),
        ),
        (
            "a provider name of 33 characters, past the length maximum 32",
            json!({
                "UserPoolId": user_pool_id,
                "ProviderName": "p".repeat(33),
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
            }),
        ),
        (
            "a provider name holding a space, which the pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "ProviderName": "two words",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
            }),
        ),
        (
            "51 IdP identifiers, past the list maximum 50",
            json!({
                "UserPoolId": user_pool_id,
                "ProviderName": "TooManyIdentifiers",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
                "IdpIdentifiers": over_long_identifiers,
            }),
        ),
        (
            "an IdP identifier of 41 characters, past the member length maximum 40",
            json!({
                "UserPoolId": user_pool_id,
                "ProviderName": "OverLongIdentifier",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
                "IdpIdentifiers": ["i".repeat(41)],
            }),
        ),
        (
            "a fifth ACR level, past the map maximum 4 and outside the key pattern",
            json!({
                "UserPoolId": user_pool_id,
                "ProviderName": "TooManyAcrLevels",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
                "AcrMapping": {
                    "Level1": "urn:acr:1",
                    "Level2": "urn:acr:2",
                    "Level3": "urn:acr:3",
                    "Level4": "urn:acr:4",
                    "Level5": "urn:acr:5",
                },
            }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({
                "UserPoolId": "no-underscore",
                "ProviderName": "AuthExampleOIDC",
                "ProviderType": "OIDC",
                "ProviderDetails": { "oidc_issuer": "https://auth.example.com" },
            }),
        ),
    ] {
        let (status, payload) = call(&router, "CreateIdentityProvider", &body).await;
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

    // None of the refused requests stored anything.
    for (label, provider_name) in [
        ("too many identifiers", "TooManyIdentifiers"),
        ("an over-long identifier", "OverLongIdentifier"),
        ("too many ACR levels", "TooManyAcrLevels"),
    ] {
        let (status, payload) = call(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": provider_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: a refused create stores no provider, got {payload}"
        );
    }
}
