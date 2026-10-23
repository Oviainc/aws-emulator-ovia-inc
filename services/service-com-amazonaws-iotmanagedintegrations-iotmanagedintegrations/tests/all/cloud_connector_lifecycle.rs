// SPDX-License-Identifier: CC0-1.0

//! The C2C (cloud-to-cloud) connector, end to end over the wire:
//! `CreateCloudConnector` -> `GetCloudConnector`.
//!
//! RULE: `CreateCloudConnector` (POST `/cloud-connectors`, success 201) stores a NEW
//! cloud-connector record carrying every member the request supplied and answers with the
//! `CloudConnectorId` it generated for it; `GetCloudConnector` (GET
//! `/cloud-connectors/{Identifier}`, `@readonly`) projects that STORED record onto its
//! output shape, so what a create wrote is exactly what a read reports — a member the
//! create did not name stays absent, and `Type` reports the kind of connector this API
//! creates (`UNLISTED`, the connector an account creates for itself, against the `LISTED`
//! connectors no operation of this service creates). A retried create carrying the same
//! `ClientToken` (`@idempotencyToken`) answers with the first call's connector instead of
//! storing a second one, an identifier no create stored answers the modelled
//! `ResourceNotFoundException` (HTTP 404), and a connector is regional: only the region
//! whose endpoint created it resolves it.
//!
//! The recorded units this was derived from (`iot-managed-integrations.`
//! `cloud-connector-lifecycle` and `…account-association-lifecycle-and-refresh-conflict`)
//! are ONE row of the tables below. The rows differ along what the rule generalises over:
//! which optional members the create names, the form and length of `Name` and
//! `Description` inside their modelled `@pattern`/`@length`, the form of the Lambda ARN
//! the `LambdaArn` `@pattern` admits (bare function name, qualified by `$LATEST` or by an
//! alias, full ARN), the form of the `ClientToken`, and the region the request is signed
//! for.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::{
    EndpointConfig, LambdaConfig, CLOUD_CONNECTOR_TYPE_VALUES, ENDPOINT_TYPE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    CloudConnector, CLOUD_CONNECTOR_RESOURCE_TYPE, CREATED_CLOUD_CONNECTOR_TYPE,
    MAX_CLOUD_CONNECTORS,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), which
/// the full-form Lambda ARN below is built for.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The collection route `CreateCloudConnector` is bound to
/// (`smithy_query(query="operation",operation="CreateCloudConnector")`).
const COLLECTION: &str = "/cloud-connectors";

/// The `@length` maximum of `DisplayName`, in characters
/// (`smithy_query(query="operation",operation="CreateCloudConnector")` -> `Name`
/// len 1..128).
const DISPLAY_NAME_MAX_LENGTH: usize = 128;

/// The `@length` maximum of `CloudConnectorDescription`, in characters (same call ->
/// `Description` len 1..256).
const DESCRIPTION_MAX_LENGTH: usize = 256;

/// The `@length` bounds of `CloudConnectorId`, in characters (same call -> `Id`
/// len 1..64).
const CLOUD_CONNECTOR_ID_LENGTH: (usize, usize) = (1, 64);

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

    /// The `arn` of the `lambda` endpoint the answer reports.
    fn lambda_arn(&self) -> Option<&str> {
        self.member("EndpointConfig")?
            .get("lambda")?
            .get("arn")?
            .as_str()
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

/// One row of the create/read table: which members the create names, and in which form.
struct Case {
    /// What an assertion calls this row.
    label: &'static str,
    name: String,
    description: Option<String>,
    /// `EndpointType`, absent on a row that names none.
    endpoint_type: Option<&'static str>,
    /// The `arn` of the `lambda` endpoint configuration.
    lambda_arn: String,
}

impl Case {
    /// The `CreateCloudConnectorRequest` body this row sends: `Name` and `EndpointConfig`
    /// are the model's required members, the rest are present only where the row names
    /// them.
    fn body(&self) -> Value {
        let mut body = json!({
            "Name": self.name,
            "EndpointConfig": { "lambda": { "arn": self.lambda_arn } },
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(endpoint_type) = self.endpoint_type {
            body["EndpointType"] = json!(endpoint_type);
        }
        body
    }
}

/// The table the create/read rule is checked over: the rows differ along the optional
/// members named, the lengths of `Name` and `Description` at both modelled bounds, and the
/// four forms the `LambdaArn` `@pattern` admits — a bare function name, one qualified by
/// `$LATEST`, one qualified by an alias, and a full function ARN.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "bare-function-name",
            name: "partner cloud connector".to_string(),
            description: None,
            endpoint_type: None,
            lambda_arn: "connector-handler".to_string(),
        },
        Case {
            label: "full-arn-with-every-member",
            name: "C2C Connector One".to_string(),
            description: Some("Lamps and switches from the partner cloud".to_string()),
            endpoint_type: Some("LAMBDA"),
            lambda_arn: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:connector-handler"),
        },
        Case {
            label: "shortest-members-qualified-latest",
            name: "c".to_string(),
            description: Some("d".to_string()),
            endpoint_type: Some("LAMBDA"),
            lambda_arn: "c:$LATEST".to_string(),
        },
        Case {
            label: "longest-members-qualified-alias",
            name: "n".repeat(DISPLAY_NAME_MAX_LENGTH),
            description: Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
            endpoint_type: None,
            lambda_arn: "connector-handler:production".to_string(),
        },
        Case {
            label: "punctuation-the-patterns-admit",
            name: "Connector-9_name here".to_string(),
            description: Some("9_dash-and space".to_string()),
            endpoint_type: Some("LAMBDA"),
            lambda_arn: "connector_handler-9".to_string(),
        },
    ]
}

/// Create one connector, asserting the create itself, and answer its `Id`.
async fn create(router: &MultiServiceRouter, case: &Case) -> String {
    let created = call(router, "POST", COLLECTION, Some(case.body())).await;
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
    let (min, max) = CLOUD_CONNECTOR_ID_LENGTH;
    assert!(
        (min..=max).contains(&id.chars().count())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "case {:?}: the Id must satisfy CloudConnectorId (len {min}..{max}, \
         ^[A-Za-z0-9-_]+$), got {id:?}",
        case.label
    );
    id
}

/// RULE (round trip): a read reports the stored record, so every member a create named
/// comes back unchanged, a member it did not name stays absent, and `Type` is the kind of
/// connector this API creates.
#[tokio::test]
async fn a_read_reports_what_the_create_stored() {
    let router = emulator();
    let mut ids = BTreeSet::new();

    for case in cases() {
        let id = create(&router, &case).await;
        assert!(
            ids.insert(id.clone()),
            "case {:?}: every create must mint its own id, {id:?} was handed out twice",
            case.label
        );

        let read = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
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
            read.string("Name"),
            Some(case.name.as_str()),
            "case {:?}: expected the stored Name {:?}, got {:?}",
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
            read.string("EndpointType"),
            case.endpoint_type,
            "case {:?}: expected EndpointType {:?}, got {:?}",
            case.label,
            case.endpoint_type,
            read.string("EndpointType")
        );
        assert_eq!(
            read.lambda_arn(),
            Some(case.lambda_arn.as_str()),
            "case {:?}: expected the stored lambda arn {:?}, got {:?}",
            case.label,
            case.lambda_arn,
            read.lambda_arn()
        );
        assert_eq!(
            read.string("Type"),
            Some(CREATED_CLOUD_CONNECTOR_TYPE),
            "case {:?}: expected Type {CREATED_CLOUD_CONNECTOR_TYPE:?}, got {:?}",
            case.label,
            read.string("Type")
        );
        assert!(
            CLOUD_CONNECTOR_TYPE_VALUES.contains(&CREATED_CLOUD_CONNECTOR_TYPE),
            "case {:?}: the reported Type must be a modelled CloudConnectorType, \
             {CLOUD_CONNECTOR_TYPE_VALUES:?} does not hold it",
            case.label
        );

        // A read changes nothing a later caller sees: reading twice answers the same.
        let again = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            again.body, read.body,
            "case {:?}: a second read must report the same record, got {:?} then {:?}",
            case.label, read.body, again.body
        );
    }
}

/// RULE: two creates that name the same members are two connectors, each with its own id —
/// only a replayed `ClientToken` makes a retry answer with the first call's connector.
#[tokio::test]
async fn the_same_request_without_a_token_creates_another_connector() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let case = Case {
        label: "twice",
        name: "duplicate name connector".to_string(),
        description: Some("Same members both times".to_string()),
        endpoint_type: Some("LAMBDA"),
        lambda_arn: "connector-handler".to_string(),
    };

    let first = create(&router, &case).await;
    let second = create(&router, &case).await;
    assert_ne!(
        first, second,
        "case \"twice\": two creates without a token are two connectors, got {first:?} twice"
    );
    assert_eq!(
        engine.state::<ServiceState>().lock().cloud_connectors.len(),
        2,
        "case \"twice\": the store must hold both connectors"
    );
}

/// RULE: a create carrying a `ClientToken` the store already holds a connector for
/// answers with THAT connector and stores nothing further, whatever form the token takes;
/// a token nothing was created under mints a new connector.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_connector() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // The forms the modelled `ClientToken` pattern admits (`^[a-zA-Z0-9=_-]+$`, len
    // 1..64): a hyphenated token, an upper-case padded one, and a single character.
    let tokens = ["retry-token-1", "TOKEN_2==", "3"];
    let mut minted = Vec::new();

    for token in tokens {
        let body = json!({
            "Name": format!("token connector {token}").replace(['=', '_'], "-"),
            "EndpointConfig": { "lambda": { "arn": "connector-handler" } },
            "ClientToken": token,
        });
        let first = call(&router, "POST", COLLECTION, Some(body.clone())).await;
        assert_eq!(
            first.status, 201,
            "case {token:?}: expected HTTP 201, got {} with body {:?}",
            first.status, first.body
        );
        let id = first
            .string("Id")
            .expect("the create answers with an Id")
            .to_string();

        // The retry repeats the SAME token and the SAME parameters — the case the model
        // documents ("retry a request that completed successfully initially using the same
        // client token and parameters") — so its answer is the first call's connector.
        let retry = call(&router, "POST", COLLECTION, Some(body)).await;
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

    let store = engine.state::<ServiceState>();
    assert_eq!(
        store.lock().cloud_connectors.len(),
        tokens.len(),
        "cases {tokens:?}: each token must have created exactly one connector"
    );
    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        tokens.len(),
        "cases {tokens:?}: a token nothing was created under mints its own connector, \
         got {minted:?}"
    );
}

/// RULE: an identifier the signing region's store does not hold is the modelled 404,
/// reporting the identifier the request carried and the kind of resource looked for —
/// and a connector belongs to the region whose endpoint created it.
#[tokio::test]
async fn a_connector_no_create_stored_in_this_region_is_not_found() {
    let router = emulator();

    // Identifiers inside the modelled `CloudConnectorId` constraints that no create
    // stored: a plain one, one at the `@length` maximum, and the shortest admitted.
    let absent = [
        "no-such-connector".to_string(),
        "x".repeat(CLOUD_CONNECTOR_ID_LENGTH.1),
        "x".to_string(),
    ];
    for identifier in &absent {
        let read = call(&router, "GET", &format!("{COLLECTION}/{identifier}"), None).await;
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
            Some(CLOUD_CONNECTOR_RESOURCE_TYPE),
            "case {identifier:?}: expected ResourceType \
             {CLOUD_CONNECTOR_RESOURCE_TYPE:?}, got {:?}",
            read.string("ResourceType")
        );
    }

    // The same identifier, through another region's endpoint: the connector created in
    // one region is not resolved by another, while its own region still reports it.
    let id = create(
        &router,
        &Case {
            label: "regional",
            name: "regional connector".to_string(),
            description: None,
            endpoint_type: Some("LAMBDA"),
            lambda_arn: "connector-handler".to_string(),
        },
    )
    .await;
    for region in ["us-west-2", "eu-central-1"] {
        let elsewhere = call_signed(
            &router,
            region,
            "GET",
            &format!("{COLLECTION}/{id}"),
            None,
        )
        .await;
        assert_eq!(
            elsewhere.status, 404,
            "case {region:?}: a connector of another region must not resolve, got {} \
             with body {:?}",
            elsewhere.status, elsewhere.body
        );
    }
    let home = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
    assert_eq!(
        home.status, 200,
        "case \"regional\": the creating region still reports it, got {} with body {:?}",
        home.status, home.body
    );
}

/// RULE: a value outside `EndpointType`'s value set is refused with the modelled
/// `ValidationException` (HTTP 400) and nothing is stored.
#[tokio::test]
async fn an_endpoint_type_outside_the_value_set_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Values the model's `EndpointType` does not declare: the one declared value in the
    // wrong case, another transport, and a value of another enum of this service.
    for rejected in ["lambda", "HTTP", "KINESIS"] {
        assert!(
            !ENDPOINT_TYPE_VALUES.contains(&rejected),
            "case {rejected:?}: the row must name a value outside \
             {ENDPOINT_TYPE_VALUES:?}"
        );
        let refused = call(
            &router,
            "POST",
            COLLECTION,
            Some(json!({
                "Name": "refused connector",
                "EndpointConfig": { "lambda": { "arn": "connector-handler" } },
                "EndpointType": rejected,
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
            .cloud_connectors
            .is_empty(),
        "a refused create must store nothing"
    );
}

/// RULE: the store is bounded — at `MAX_CLOUD_CONNECTORS` records a create is refused with
/// a modelled error rather than growing the store.
#[tokio::test]
async fn a_create_past_the_maximum_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Seed the store to its cap directly: driving the bound through the wire would be
    // thousands of requests to assert one refusal.
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        for n in 0..MAX_CLOUD_CONNECTORS {
            let id = format!("seeded-{n}");
            store.cloud_connectors.insert(
                id.clone(),
                CloudConnector {
                    id: id.clone(),
                    region: REGION.to_string(),
                    name: format!("seeded connector {n}"),
                    description: None,
                    endpoint_config: EndpointConfig {
                        lambda: Some(LambdaConfig {
                            arn: "connector-handler".to_string(),
                        }),
                    },
                    endpoint_type: Some("LAMBDA".to_string()),
                    connector_type: CREATED_CLOUD_CONNECTOR_TYPE.to_string(),
                },
            );
            store.cloud_connector_order.push(id);
        }
    }

    let refused = call(
        &router,
        "POST",
        COLLECTION,
        Some(json!({
            "Name": "one too many",
            "EndpointConfig": { "lambda": { "arn": "connector-handler" } },
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
        engine.state::<ServiceState>().lock().cloud_connectors.len(),
        MAX_CLOUD_CONNECTORS,
        "case \"at-the-maximum\": a refused create must not have grown the store"
    );
}
