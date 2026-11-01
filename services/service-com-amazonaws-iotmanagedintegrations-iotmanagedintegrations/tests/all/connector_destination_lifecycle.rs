// SPDX-License-Identifier: CC0-1.0

//! The connector destination, end to end over the wire: `CreateConnectorDestination` ->
//! `GetConnectorDestination`.
//!
//! RULE: `CreateConnectorDestination` (POST `/connector-destinations`, success 201)
//! RESOLVES the `CloudConnectorId` its request names against the cloud connectors the
//! signing region holds and refuses, with the modelled `ValidationException` (HTTP 400)
//! and no record stored, any create naming a connector that region does not hold —
//! whatever the form of that identifier; for a connector it does hold it stores a NEW
//! connector-destination record carrying every member the request supplied and answers
//! with the `ConnectorDestinationId` it generated. `GetConnectorDestination` (GET
//! `/connector-destinations/{Identifier}`, `@readonly`) projects that STORED record onto
//! its output shape, so what a create wrote is exactly what a read reports, and a member
//! the create did not name stays absent. A retried create carrying the same `ClientToken`
//! (`@idempotencyToken`) answers with the first call's destination instead of storing a
//! second one, an identifier no create stored answers the modelled
//! `ResourceNotFoundException` (HTTP 404) — from a read and from an update —
//! `UpdateConnectorDestination` applies the members its request names and leaves the rest
//! as they were, and `DeleteConnectorDestination` removes the record the read reported,
//! reporting the state the caller asked for (HTTP 200) when there is no record left to
//! remove. A
//! destination is regional: only the region whose endpoint created it resolves, updates or
//! deletes it — and only that region's connectors can be named by a create.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.connector-destination-referential-integrity`) is ONE row of
//! the tables below. The rows differ along what the rule generalises over: which optional
//! members the create names, the form and length of `Name` and `Description` inside their
//! modelled `@pattern`/`@length`, which authentication configuration the `AuthConfig`
//! carries (none, OAuth, one or three General Authorization materials) and how many
//! materials it lists, the form and length of the identifier a refused create names, and
//! the region the request is signed for.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::{
    AuthConfig, AUTH_TYPE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    cannot_be_found_message, resource_name_not_found_message, unknown_reference_message,
    ConnectorDestination, CONNECTOR_DESTINATION_RESOURCE_NAME,
    CONNECTOR_DESTINATION_RESOURCE_SUBJECT, CONNECTOR_DESTINATION_RESOURCE_TYPE,
    MAX_CONNECTOR_DESTINATIONS,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), which
/// the Secrets Manager ARNs below are built for (the `SecretsManagerArn` `@pattern`
/// requires 12 digits there).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The collection route `CreateConnectorDestination` is bound to
/// (`smithy_query(query="operation",operation="CreateConnectorDestination")`).
const DESTINATIONS: &str = "/connector-destinations";

/// The collection route `CreateCloudConnector` is bound to — the connector a destination
/// has to name.
const CONNECTORS: &str = "/cloud-connectors";

/// The `@length` bounds of `ConnectorDestinationId`, in characters
/// (`smithy_query(query="shape",shape="ConnectorDestinationId")` -> len 1..64).
const DESTINATION_ID_LENGTH: (usize, usize) = (1, 64);

/// The `@length` bounds of `CloudConnectorId`, in characters
/// (`smithy_query(query="operation",operation="CreateConnectorDestination")` ->
/// `CloudConnectorId` len 1..64).
const CLOUD_CONNECTOR_ID_LENGTH: (usize, usize) = (1, 64);

/// The `@length` maximum of `ConnectorDestinationName`, in characters (same call ->
/// `Name` len 1..128).
const NAME_MAX_LENGTH: usize = 128;

/// The `@length` maximum of `ConnectorDestinationDescription`, in characters (same call ->
/// `Description` len 1..256).
const DESCRIPTION_MAX_LENGTH: usize = 256;

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the caller keeps a handle to, so a test can seed or
/// inspect the service's store directly instead of driving thousands of requests.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and reduce the answer. The `Authorization` credential scope is what
/// routes the request to this service and names the region it is signed for.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_signed(router, REGION, method, path_and_query, body).await
}

/// [`call`], signed for an explicit region, so a regional resource can be addressed
/// through another region's endpoint.
async fn call_signed(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// Create a C2C connector in `region` through the wire and answer its `CloudConnectorId`:
/// the resource a connector destination has to name.
async fn cloud_connector(router: &MultiServiceRouter, region: &str) -> String {
    let created = call_signed(
        router,
        region,
        "POST",
        CONNECTORS,
        Some(json!({
            "Name": "partner cloud connector",
            "EndpointConfig": { "lambda": { "arn": "connector-handler" } },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding a cloud connector in {region:?}: expected HTTP 201, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .expect("the connector create answers with an Id")
        .to_string()
}

/// One `SecretsManager` configuration, inside the `SecretsManagerArn` `@pattern`
/// (`^arn:aws:secretsmanager:[0-9a-zA-Z-]{1,32}:\d{12}:secret:[A-Za-z0-9/_+=.@-]{8,520}$`)
/// and the `SecretsManagerVersionId` `@length` (32..64).
fn secret(name: &str, version_nibble: char) -> Value {
    json!({
        "arn": format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{name}-A1b2C3"),
        "versionId": std::iter::repeat_n(version_nibble, 32).collect::<String>(),
    })
}

/// One `AuthMaterial` of a General Authorization `AuthConfig`: a secret plus the
/// `AuthMaterialName` (len 1..100, `^[a-zA-Z0-9=_+/-]+$`) it is held under.
fn auth_material(name: &str, version_nibble: char) -> Value {
    json!({
        "SecretsManager": secret(name, version_nibble),
        "AuthMaterialName": name,
    })
}

/// One row of the create/read table: which members the create names, and in which form.
struct Case {
    /// What an assertion calls this row.
    label: &'static str,
    name: Option<String>,
    description: Option<String>,
    /// `AuthType`, absent on a row that names none.
    auth_type: Option<&'static str>,
    /// The `@required` `AuthConfig`, exactly as the row sends it.
    auth_config: Value,
    /// `SecretsManager`, absent on a row that names none.
    secrets_manager: Option<Value>,
}

impl Case {
    /// The `CreateConnectorDestinationRequest` body this row sends against
    /// `cloud_connector_id`: `CloudConnectorId` and `AuthConfig` are the model's required
    /// members, the rest are present only where the row names them.
    fn body(&self, cloud_connector_id: &str) -> Value {
        let mut body = json!({
            "CloudConnectorId": cloud_connector_id,
            "AuthConfig": self.auth_config,
        });
        if let Some(name) = &self.name {
            body["Name"] = json!(name);
        }
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(auth_type) = self.auth_type {
            body["AuthType"] = json!(auth_type);
        }
        if let Some(secrets_manager) = &self.secrets_manager {
            body["SecretsManager"] = secrets_manager.clone();
        }
        body
    }

    /// The `OAuthCompleteRedirectUrl` a read of this row's destination must report: the
    /// URL its OAuth configuration names, and only where that value satisfies the OUTPUT
    /// member's own modelled constraints (`OAuthCompleteRedirectUrl`, len 1..1024,
    /// `^(http|https)://.*$`) — the member it is supplied on targets an unconstrained
    /// `String`, so a value outside them stays on the `AuthConfig` the read reports.
    fn expected_redirect_url(&self) -> Option<&str> {
        self.auth_config
            .get("oAuth")?
            .get("oAuthCompleteRedirectUrl")?
            .as_str()
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
    }
}

/// The table the create/read rule is checked over: the rows differ along the optional
/// members named, the lengths of `Name` and `Description` at both modelled bounds, and the
/// authentication configurations `AuthConfig` admits — none at all, a full OAuth
/// configuration, a minimal one carrying only `OAuthConfig`'s three required members, and
/// General Authorization with one and with the three materials the `AuthMaterials`
/// `@length` admits.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "minimal-empty-auth-config",
            name: None,
            description: None,
            auth_type: None,
            auth_config: json!({}),
            secrets_manager: None,
        },
        Case {
            label: "oauth-with-every-member",
            name: Some("Partner Destination".to_string()),
            description: Some("Lamps and switches from the partner cloud".to_string()),
            auth_type: Some("OAUTH"),
            auth_config: json!({
                "oAuth": {
                    "authUrl": "https://partner.example.com/oauth2/authorize",
                    "tokenUrl": "https://partner.example.com/oauth2/token",
                    "scope": "devices.read devices.write",
                    "tokenEndpointAuthenticationScheme": "HTTP_BASIC",
                    "oAuthCompleteRedirectUrl": "https://partner.example.com/oauth2/done",
                    // `DaysBeforeRenewal` has a modelled minimum of 30.
                    "proactiveRefreshTokenRenewal": { "enabled": true, "DaysBeforeRenewal": 30 },
                },
            }),
            secrets_manager: Some(secret("partner-oauth", 'a')),
        },
        Case {
            label: "oauth-required-members-only",
            name: Some("c".to_string()),
            description: Some("d".to_string()),
            auth_type: Some("OAUTH"),
            auth_config: json!({
                "oAuth": {
                    "authUrl": "http://partner.example.org/authorize",
                    "tokenUrl": "http://partner.example.org/token",
                    "tokenEndpointAuthenticationScheme": "REQUEST_BODY_CREDENTIALS",
                },
            }),
            secrets_manager: None,
        },
        Case {
            label: "general-authorization-one-material",
            name: Some("n".repeat(NAME_MAX_LENGTH)),
            description: Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
            auth_type: None,
            auth_config: json!({
                "GeneralAuthorization": [auth_material("partner-key", 'b')],
            }),
            secrets_manager: Some(secret("partner-general", 'c')),
        },
        Case {
            label: "general-authorization-three-materials",
            name: Some("Destination-9_name here".to_string()),
            description: Some("9_dash-and space".to_string()),
            auth_type: None,
            // `AuthMaterials` is a list with a modelled `@length` maximum of 3.
            auth_config: json!({
                "GeneralAuthorization": [
                    auth_material("key-one", 'd'),
                    auth_material("key_two", 'e'),
                    auth_material("key/three", 'f'),
                ],
            }),
            secrets_manager: None,
        },
        Case {
            label: "redirect-url-the-output-member-cannot-carry",
            name: Some("Native redirect destination".to_string()),
            description: None,
            auth_type: Some("OAUTH"),
            auth_config: json!({
                "oAuth": {
                    "authUrl": "https://partner.example.com/oauth2/authorize",
                    "tokenUrl": "https://partner.example.com/oauth2/token",
                    "tokenEndpointAuthenticationScheme": "HTTP_BASIC",
                    // A mobile app's own scheme: admitted where it is supplied (an
                    // unconstrained `String`), outside `OAuthCompleteRedirectUrl`'s
                    // `^(http|https)://.*$`.
                    "oAuthCompleteRedirectUrl": "partnerapp://oauth/done",
                },
            }),
            secrets_manager: None,
        },
    ]
}

/// Create one destination against `cloud_connector_id`, asserting the create itself, and
/// answer its `Id`.
async fn create(router: &MultiServiceRouter, case: &Case, cloud_connector_id: &str) -> String {
    let created = call(
        router,
        "POST",
        DESTINATIONS,
        Some(case.body(cloud_connector_id)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected HTTP 201, got {} with body {:?}",
        case.label, created.status, created.body
    );
    let id = created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected an Id member, got {:?}",
                case.label, created.body
            )
        })
        .to_string();
    let (min, max) = DESTINATION_ID_LENGTH;
    assert!(
        (min..=max).contains(&id.chars().count())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "case {:?}: the Id must satisfy ConnectorDestinationId (len {min}..{max}, \
         ^[A-Za-z0-9-_]+$), got {id:?}",
        case.label
    );
    id
}

/// RULE (round trip): a read reports the stored record, so every member a create named
/// comes back unchanged — including the whole `AuthConfig` — and a member it did not name
/// stays absent.
#[tokio::test]
async fn a_read_reports_what_the_create_stored() {
    let router = emulator();
    let connector = cloud_connector(&router, REGION).await;
    let mut ids = BTreeSet::new();

    for case in cases() {
        let id = create(&router, &case, &connector).await;
        assert!(
            ids.insert(id.clone()),
            "case {:?}: every create must mint its own id, {id:?} was handed out twice",
            case.label
        );

        let read = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200, got {} with body {:?}",
            case.label, read.status, read.body
        );
        assert_eq!(
            read.string("Id"),
            Some(id.as_str()),
            "case {:?}: expected the created id {id:?}, got {:?}",
            case.label,
            read.string("Id")
        );
        assert_eq!(
            read.string("CloudConnectorId"),
            Some(connector.as_str()),
            "case {:?}: expected the named CloudConnectorId {connector:?}, got {:?}",
            case.label,
            read.string("CloudConnectorId")
        );
        assert_eq!(
            read.string("Name"),
            case.name.as_deref(),
            "case {:?}: expected Name {:?}, got {:?}",
            case.label,
            case.name,
            read.string("Name")
        );
        assert_eq!(
            read.string("Description"),
            case.description.as_deref(),
            "case {:?}: expected Description {:?}, got {:?}",
            case.label,
            case.description,
            read.string("Description")
        );
        assert_eq!(
            read.string("AuthType"),
            case.auth_type,
            "case {:?}: expected AuthType {:?}, got {:?}",
            case.label,
            case.auth_type,
            read.string("AuthType")
        );
        assert_eq!(
            read.member("AuthConfig"),
            Some(&case.auth_config),
            "case {:?}: expected the stored AuthConfig {:?}, got {:?}",
            case.label,
            case.auth_config,
            read.member("AuthConfig")
        );
        assert_eq!(
            read.member("SecretsManager"),
            case.secrets_manager.as_ref(),
            "case {:?}: expected SecretsManager {:?}, got {:?}",
            case.label,
            case.secrets_manager,
            read.member("SecretsManager")
        );
        assert_eq!(
            read.string("OAuthCompleteRedirectUrl"),
            case.expected_redirect_url(),
            "case {:?}: expected OAuthCompleteRedirectUrl {:?}, got {:?}",
            case.label,
            case.expected_redirect_url(),
            read.string("OAuthCompleteRedirectUrl")
        );

        // A read changes nothing a later caller sees: reading twice answers the same.
        let again = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            again.body, read.body,
            "case {:?}: a second read must report the same record, got {:?} then {:?}",
            case.label, read.body, again.body
        );
    }
}

/// RULE (referential integrity): a create naming a `CloudConnectorId` the signing region
/// holds no connector for is refused with the modelled `ValidationException` (HTTP 400)
/// and stores nothing, whatever the form or length of that identifier — including the id
/// of a connector another region's endpoint created. The same request against a connector
/// this region does hold is accepted, so the refusal is about the connector's EXISTENCE
/// and not about the request's shape.
#[tokio::test]
async fn a_destination_for_a_connector_this_region_does_not_hold_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // A connector that exists, but in ANOTHER region: an identifier this service really
    // handed out, which this region's endpoint still must not resolve.
    let elsewhere = cloud_connector(&router, "eu-central-1").await;
    // Identifiers inside the modelled `CloudConnectorId` constraints that no create
    // stored here: a plain one, the shortest admitted, one at the `@length` maximum, one
    // whose characters are the punctuation the pattern admits, and the out-of-region id.
    let absent = [
        ("plain", "no-such-connector".to_string()),
        ("shortest", "x".to_string()),
        ("longest", "x".repeat(CLOUD_CONNECTOR_ID_LENGTH.1)),
        ("punctuation", "Connector-9_id".to_string()),
        ("another-region", elsewhere),
    ];

    let expected_message = unknown_reference_message("CloudConnectorId");
    for (label, connector) in &absent {
        assert!(
            (CLOUD_CONNECTOR_ID_LENGTH.0..=CLOUD_CONNECTOR_ID_LENGTH.1)
                .contains(&connector.chars().count()),
            "case {label:?}: the row must name an identifier inside CloudConnectorId's \
             modelled length, got {connector:?}"
        );
        for case in cases() {
            let refused = call(
                &router,
                "POST",
                DESTINATIONS,
                Some(case.body(connector)),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {label:?}/{:?}: expected HTTP 400, got {} with body {:?}",
                case.label, refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ValidationException"),
                "case {label:?}/{:?}: expected the modelled refusal, got {:?}",
                case.label,
                refused.error_type
            );
            let message = refused.string("Message");
            // The reason is the ONE sentence this service words a bad cross-resource
            // reference with (`state::unknown_reference_message`, recorded on this very
            // refusal as `The provided CloudConnectorId does not exist.`), the same for
            // every row: it names the member at fault and quotes no identifier back — so
            // an identifier the model marks `@sensitive` could never be published through
            // it either.
            assert_eq!(
                message,
                Some(expected_message.as_str()),
                "case {label:?}/{:?}: expected the member-naming reason \
                 {expected_message:?}, got {message:?}",
                case.label
            );
        }
    }
    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .is_empty(),
        "cases {absent:?}: a refused create must store nothing"
    );

    // The same rows, against a connector this region DOES hold: accepted.
    let here = cloud_connector(&router, REGION).await;
    for case in cases() {
        create(&router, &case, &here).await;
    }
    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .len(),
        cases().len(),
        "every create against an existing connector must have stored its destination"
    );
}

/// RULE: two creates that name the same members are two destinations, each with its own
/// id — only a replayed `ClientToken` makes a retry answer with the first call's
/// destination, whatever form the token takes; a token nothing was created under mints a
/// new one.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_destination() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let connector = cloud_connector(&router, REGION).await;

    // Two creates with no token at all are two destinations.
    let case = Case {
        label: "twice",
        name: Some("duplicate destination".to_string()),
        description: None,
        auth_type: None,
        auth_config: json!({}),
        secrets_manager: None,
    };
    let first = create(&router, &case, &connector).await;
    let second = create(&router, &case, &connector).await;
    assert_ne!(
        first, second,
        "case \"twice\": two creates without a token are two destinations, got \
         {first:?} twice"
    );

    // The forms the modelled `ClientToken` pattern admits (`^[a-zA-Z0-9=_-]+$`, len
    // 1..64): a hyphenated token, an upper-case padded one, a single character, and one at
    // the length maximum.
    let tokens = ["retry-token-1", "TOKEN_2==", "3", &"t".repeat(64)];
    let mut minted = Vec::new();
    for token in tokens {
        let mut body = case.body(&connector);
        body["ClientToken"] = json!(token);

        let created = call(&router, "POST", DESTINATIONS, Some(body.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {token:?}: expected HTTP 201, got {} with body {:?}",
            created.status, created.body
        );
        let id = created
            .string("Id")
            .expect("the create answers with an Id")
            .to_string();

        // The retry repeats the SAME token and the SAME parameters — the case the model
        // documents ("retry a request that completed successfully initially using the same
        // client token and parameters") — so its answer is the first call's destination.
        let retry = call(&router, "POST", DESTINATIONS, Some(body)).await;
        assert_eq!(
            retry.status, 201,
            "case {token:?}: a replay is still accepted, got {} with body {:?}",
            retry.status, retry.body
        );
        assert_eq!(
            retry.string("Id"),
            Some(id.as_str()),
            "case {token:?}: a replayed token must answer with {id:?}, got {:?}",
            retry.string("Id")
        );
        minted.push(id);
    }

    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        tokens.len(),
        "cases {tokens:?}: a token nothing was created under mints its own destination, \
         got {minted:?}"
    );
    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .len(),
        tokens.len() + 2,
        "cases {tokens:?}: each token must have created exactly one destination, beside \
         the two untokened creates"
    );
}

/// RULE: an identifier the signing region's store does not hold is the modelled 404,
/// reporting the identifier the request carried and the kind of resource looked for — and
/// a destination belongs to the region whose endpoint created it.
#[tokio::test]
async fn a_destination_no_create_stored_in_this_region_is_not_found() {
    let router = emulator();

    // Identifiers inside the modelled `ConnectorDestinationId` constraints that no create
    // stored: a plain one, one at the `@length` maximum, and the shortest admitted.
    let absent = [
        "no-such-destination".to_string(),
        "x".repeat(DESTINATION_ID_LENGTH.1),
        "x".to_string(),
    ];
    for identifier in &absent {
        let read = call(&router, "GET", &format!("{DESTINATIONS}/{identifier}"), None).await;
        assert_eq!(
            read.status, 404,
            "case {identifier:?}: expected HTTP 404, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {identifier:?}: expected the modelled 404, got {:?}",
            read.error_type
        );
        assert_eq!(
            read.string("ResourceId"),
            Some(identifier.as_str()),
            "case {identifier:?}: the 404 reports the identifier the request carried, \
             got {:?}",
            read.string("ResourceId")
        );
        assert_eq!(
            read.string("ResourceType"),
            Some(CONNECTOR_DESTINATION_RESOURCE_TYPE),
            "case {identifier:?}: expected ResourceType \
             {CONNECTOR_DESTINATION_RESOURCE_TYPE:?}, got {:?}",
            read.string("ResourceType")
        );
        assert_eq!(
            read.string("Message"),
            Some(resource_name_not_found_message(CONNECTOR_DESTINATION_RESOURCE_NAME).as_str()),
            "case {identifier:?}: expected the read path's reason, got {:?}",
            read.string("Message")
        );
    }

    // The same identifier, through another region's endpoint: the destination created in
    // one region is not resolved by another, while its own region still reports it.
    let connector = cloud_connector(&router, REGION).await;
    let id = create(
        &router,
        &Case {
            label: "regional",
            name: Some("regional destination".to_string()),
            description: None,
            auth_type: None,
            auth_config: json!({}),
            secrets_manager: None,
        },
        &connector,
    )
    .await;
    for region in ["us-west-2", "eu-central-1"] {
        let there = call_signed(
            &router,
            region,
            "GET",
            &format!("{DESTINATIONS}/{id}"),
            None,
        )
        .await;
        assert_eq!(
            there.status, 404,
            "case {region:?}: a destination of another region must not resolve, got {} \
             with body {:?}",
            there.status, there.body
        );
    }
    let home = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
    assert_eq!(
        home.status, 200,
        "case \"regional\": the creating region still reports it, got {} with body {:?}",
        home.status, home.body
    );
}

/// RULE: a delete removes the stored record, so the read that reported it answers the
/// modelled 404 afterwards and the identifier is gone from the store; a delete of an
/// identifier the signing region holds no record for reports the state the caller asked
/// for instead of a refusal, and a destination of another region is not this region's to
/// remove, so such a call leaves it where it is.
#[tokio::test]
async fn a_delete_removes_the_record_the_read_reported() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let connector = cloud_connector(&router, REGION).await;

    for case in cases() {
        let id = create(&router, &case, &connector).await;
        let read = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the created destination must read back before the delete, got {}",
            case.label, read.status
        );

        // Another region's endpoint must not delete it.
        let elsewhere = call_signed(
            &router,
            "eu-central-1",
            "DELETE",
            &format!("{DESTINATIONS}/{id}"),
            None,
        )
        .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {:?}: a delete this region holds no record for is a success, got {} \
             with body {:?}",
            case.label, elsewhere.status, elsewhere.body
        );
        let untouched = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            untouched.status, 200,
            "case {:?}: another region's delete must leave the record where it is, got {} \
             with body {:?}",
            case.label, untouched.status, untouched.body
        );

        let deleted = call(&router, "DELETE", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected HTTP 200, got {} with body {:?}",
            case.label, deleted.status, deleted.body
        );

        let gone = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            gone.status, 404,
            "case {:?}: the deleted destination must not read back, got {} with body {:?}",
            case.label, gone.status, gone.body
        );

        // A second delete of the same identifier asks for the state the store is already
        // in, so it is the same success as the first one and carries no error member.
        let again = call(&router, "DELETE", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: deleting it twice is a success, got {} with body {:?}",
            case.label, again.status, again.body
        );
        assert_eq!(
            again.error_type, None,
            "case {:?}: a successful delete carries no error code, got {:?}",
            case.label, again.error_type
        );
        assert_eq!(
            again.string("Message"),
            None,
            "case {:?}: a successful delete carries no reason, got {:?}",
            case.label,
            again.string("Message")
        );
    }

    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .is_empty(),
        "every created destination must have been removed by its delete"
    );
    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destination_order
            .is_empty(),
        "a delete must drop the id from the insertion order too"
    );
}

/// RULE: an update applies the members its request names to the stored record and leaves
/// every member it does not name as it was, so a read after it reports the merge of the two
/// — and an identifier the signing region holds no record for is the modelled 404.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_names() {
    let router = emulator();
    let connector = cloud_connector(&router, REGION).await;

    // The rows differ along WHICH members the update names: one member alone, several at
    // once, and an update that names none of them (a no-op merge).
    let updates: [(&str, Value); 4] = [
        ("name-only", json!({ "Name": "Renamed destination" })),
        (
            "description-only",
            json!({ "Description": "A new description" }),
        ),
        (
            "name-description-and-secret",
            json!({
                "Name": "Renamed again",
                "Description": "And described again",
                "SecretsManager": secret("rotated-secret", '9'),
            }),
        ),
        ("nothing-at-all", json!({})),
    ];

    for case in cases() {
        for (label, update) in &updates {
            let id = create(&router, &case, &connector).await;
            let before = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;

            let updated = call(
                &router,
                "PUT",
                &format!("{DESTINATIONS}/{id}"),
                Some(update.clone()),
            )
            .await;
            assert_eq!(
                updated.status, 204,
                "case {:?}/{label:?}: expected HTTP 204, got {} with body {:?}",
                case.label, updated.status, updated.body
            );

            let after = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
            assert_eq!(
                after.status, 200,
                "case {:?}/{label:?}: the updated destination must read back, got {}",
                case.label, after.status
            );
            // Every member of the read: the update's value where it named one, the value
            // the create stored where it did not.
            for member in [
                "Id",
                "CloudConnectorId",
                "Name",
                "Description",
                "AuthType",
                "AuthConfig",
                "SecretsManager",
                "OAuthCompleteRedirectUrl",
            ] {
                let expected = update.get(member).or_else(|| before.member(member));
                assert_eq!(
                    after.member(member),
                    expected,
                    "case {:?}/{label:?}: expected {member} {expected:?}, got {:?}",
                    case.label,
                    after.member(member)
                );
            }
        }
    }

    // An identifier this region holds no record for — never created, or created and then
    // deleted — is the modelled 404, worded from the identifier the request carried.
    let id = create(&router, &cases()[0], &connector).await;
    let deleted = call(&router, "DELETE", &format!("{DESTINATIONS}/{id}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "the destination must delete before the update, got {} with body {:?}",
        deleted.status, deleted.body
    );
    for identifier in [id.as_str(), "no-such-destination"] {
        let refused = call(
            &router,
            "PUT",
            &format!("{DESTINATIONS}/{identifier}"),
            Some(json!({ "Name": "Renamed destination" })),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {identifier:?}: expected HTTP 404, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {identifier:?}: expected the modelled 404, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.string("Message"),
            Some(cannot_be_found_message(CONNECTOR_DESTINATION_RESOURCE_SUBJECT).as_str()),
            "case {identifier:?}: expected the mutating path's reason, got {:?}",
            refused.string("Message")
        );
    }
}

/// RULE: an update of the `AuthConfig` writes each General Authorization material under its
/// `AuthMaterialName` — replacing the material held under that name, appending one the list
/// does not hold — and is refused with the modelled `ValidationException` where appending
/// would take the list past the modelled maximum of three; an `oAuthUpdate` against a
/// destination that carries no OAuth configuration is refused the same way.
#[tokio::test]
async fn an_auth_config_update_writes_each_material_under_its_name() {
    let router = emulator();
    let connector = cloud_connector(&router, REGION).await;

    // A destination holding one material: an update REPLACES it under the same name
    // through either list, and APPENDS the ones it does not hold.
    for member in ["AuthMaterialsToAdd", "AuthMaterialsToUpdate"] {
        let id = create(
            &router,
            &Case {
                label: "one-material",
                name: None,
                description: None,
                auth_type: None,
                auth_config: json!({
                    "GeneralAuthorization": [auth_material("partner-key", 'b')],
                }),
                secrets_manager: None,
            },
            &connector,
        )
        .await;

        let rotated = auth_material("partner-key", '7');
        let added = auth_material("second-key", '8');
        let updated = call(
            &router,
            "PUT",
            &format!("{DESTINATIONS}/{id}"),
            Some(json!({
                "AuthConfig": {
                    "GeneralAuthorizationUpdate": { member: [rotated, added] },
                },
            })),
        )
        .await;
        assert_eq!(
            updated.status, 204,
            "case {member:?}: expected HTTP 204, got {} with body {:?}",
            updated.status, updated.body
        );

        let read = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            read.member("AuthConfig"),
            Some(&json!({
                "GeneralAuthorization": [
                    auth_material("partner-key", '7'),
                    auth_material("second-key", '8'),
                ],
            })),
            "case {member:?}: the material named twice must be replaced in place and the \
             new one appended, got {:?}",
            read.member("AuthConfig")
        );

        // One material past the modelled maximum of three: refused, and the stored list
        // is left as it was.
        let before = read.member("AuthConfig").cloned();
        let refused = call(
            &router,
            "PUT",
            &format!("{DESTINATIONS}/{id}"),
            Some(json!({
                "AuthConfig": {
                    "GeneralAuthorizationUpdate": {
                        member: [auth_material("third-key", '1'), auth_material("fourth-key", '2')],
                    },
                },
            })),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {member:?}: expected HTTP 400 past the maximum, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {member:?}: expected the modelled refusal, got {:?}",
            refused.error_type
        );
        let after = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
        assert_eq!(
            after.member("AuthConfig").cloned(),
            before,
            "case {member:?}: a refused update must leave the stored record as it was, \
             got {:?} after {before:?}",
            after.member("AuthConfig")
        );
    }

    // An OAuth update needs an OAuth configuration to update: `OAuthUpdate` carries none
    // of `OAuthConfig`'s three required members, so there is nothing to build a missing
    // one out of.
    let id = create(
        &router,
        &Case {
            label: "no-oauth-configuration",
            name: None,
            description: None,
            auth_type: None,
            auth_config: json!({}),
            secrets_manager: None,
        },
        &connector,
    )
    .await;
    let refused = call(
        &router,
        "PUT",
        &format!("{DESTINATIONS}/{id}"),
        Some(json!({
            "AuthConfig": {
                "oAuthUpdate": { "oAuthCompleteRedirectUrl": "https://partner.example.com/done" },
            },
        })),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"no-oauth-configuration\": expected HTTP 400, got {} with body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ValidationException"),
        "case \"no-oauth-configuration\": expected the modelled refusal, got {:?}",
        refused.error_type
    );

    // The same update against a destination that DOES carry one is applied, and the
    // read's own `OAuthCompleteRedirectUrl` follows it.
    let id = create(
        &router,
        &Case {
            label: "oauth-configuration",
            name: None,
            description: None,
            auth_type: Some("OAUTH"),
            auth_config: json!({
                "oAuth": {
                    "authUrl": "https://partner.example.com/oauth2/authorize",
                    "tokenUrl": "https://partner.example.com/oauth2/token",
                    "tokenEndpointAuthenticationScheme": "HTTP_BASIC",
                },
            }),
            secrets_manager: None,
        },
        &connector,
    )
    .await;
    let updated = call(
        &router,
        "PUT",
        &format!("{DESTINATIONS}/{id}"),
        Some(json!({
            "AuthConfig": {
                "oAuthUpdate": {
                    "oAuthCompleteRedirectUrl": "https://partner.example.com/done",
                    "proactiveRefreshTokenRenewal": { "enabled": false, "DaysBeforeRenewal": 45 },
                },
            },
        })),
    )
    .await;
    assert_eq!(
        updated.status, 204,
        "case \"oauth-configuration\": expected HTTP 204, got {} with body {:?}",
        updated.status, updated.body
    );
    let read = call(&router, "GET", &format!("{DESTINATIONS}/{id}"), None).await;
    assert_eq!(
        read.member("AuthConfig"),
        Some(&json!({
            "oAuth": {
                "authUrl": "https://partner.example.com/oauth2/authorize",
                "tokenUrl": "https://partner.example.com/oauth2/token",
                "tokenEndpointAuthenticationScheme": "HTTP_BASIC",
                "oAuthCompleteRedirectUrl": "https://partner.example.com/done",
                "proactiveRefreshTokenRenewal": { "enabled": false, "DaysBeforeRenewal": 45 },
            },
        })),
        "case \"oauth-configuration\": the update must merge into the stored OAuth \
         configuration, got {:?}",
        read.member("AuthConfig")
    );
    assert_eq!(
        read.string("OAuthCompleteRedirectUrl"),
        Some("https://partner.example.com/done"),
        "case \"oauth-configuration\": the read's own redirect member follows the stored \
         configuration, got {:?}",
        read.string("OAuthCompleteRedirectUrl")
    );
}

/// RULE: a value outside `AuthType`'s value set is refused with the modelled
/// `ValidationException` (HTTP 400) and nothing is stored.
#[tokio::test]
async fn an_auth_type_outside_the_value_set_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let connector = cloud_connector(&router, REGION).await;

    // Values the model's `AuthType` does not declare: the one declared value in the wrong
    // case, a later version of the same scheme, and a value of another enum of this
    // service.
    for rejected in ["oauth", "OAUTH2", "LAMBDA"] {
        assert!(
            !AUTH_TYPE_VALUES.contains(&rejected),
            "case {rejected:?}: the row must name a value outside {AUTH_TYPE_VALUES:?}"
        );
        let refused = call(
            &router,
            "POST",
            DESTINATIONS,
            Some(json!({
                "CloudConnectorId": connector,
                "AuthConfig": {},
                "AuthType": rejected,
            })),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {rejected:?}: expected HTTP 400, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {rejected:?}: expected the modelled refusal, got {:?}",
            refused.error_type
        );
    }
    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .is_empty(),
        "a refused create must store nothing"
    );
}

/// RULE: the store is bounded — at `MAX_CONNECTOR_DESTINATIONS` records a create is
/// refused with a modelled error rather than growing the store.
#[tokio::test]
async fn a_create_past_the_maximum_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let connector = cloud_connector(&router, REGION).await;

    // Seed the store to its cap directly: driving the bound through the wire would be
    // thousands of requests to assert one refusal.
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        for n in 0..MAX_CONNECTOR_DESTINATIONS {
            let id = format!("seeded-{n}");
            store.connector_destinations.insert(
                id.clone(),
                ConnectorDestination {
                    id: id.clone(),
                    region: REGION.to_string(),
                    cloud_connector_id: connector.clone(),
                    name: Some(format!("seeded destination {n}")),
                    description: None,
                    auth_type: None,
                    auth_config: AuthConfig::default(),
                    secrets_manager: None,
                    sequence: n as u64,
                },
            );
            store.connector_destination_order.push(id);
        }
    }

    let refused = call(
        &router,
        "POST",
        DESTINATIONS,
        Some(json!({
            "CloudConnectorId": connector,
            "AuthConfig": {},
            "Name": "one too many",
        })),
    )
    .await;
    assert_eq!(
        refused.status, 429,
        "case \"at-the-maximum\": expected HTTP 429, got {} with body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ThrottlingException"),
        "case \"at-the-maximum\": expected a modelled refusal, got {:?}",
        refused.error_type
    );
    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .len(),
        MAX_CONNECTOR_DESTINATIONS,
        "case \"at-the-maximum\": a refused create must not have grown the store"
    );
}
