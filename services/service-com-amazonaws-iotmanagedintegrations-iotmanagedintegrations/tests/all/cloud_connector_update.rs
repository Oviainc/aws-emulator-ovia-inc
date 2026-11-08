// SPDX-License-Identifier: CC0-1.0

//! `UpdateCloudConnector` over the wire: `CreateCloudConnector` -> `UpdateCloudConnector`
//! -> `GetCloudConnector`.
//!
//! RULE: `UpdateCloudConnector` (PUT `/cloud-connectors/{Identifier}`, success 200, empty
//! body) applies each member the request NAMES to the stored cloud-connector record the
//! signing region holds under that identifier and leaves every member it does not name as
//! it was, so a following read reports the new values of the named members and the old
//! values of all the others; an identifier that region holds no connector under is the
//! modelled `ResourceNotFoundException` (HTTP 404) and changes nothing.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.cloud-connector-lifecycle`) is ONE row of the table below.
//! The rows differ along what the rule generalises over: WHICH of the two updatable
//! members the request names (`Name` alone, `Description` alone, both, neither), and the
//! form and length of the values inside their modelled `@pattern`/`@length` bounds — at
//! the shortest admitted, at the maximum, and with the punctuation each pattern admits.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    CLOUD_CONNECTOR_RESOURCE_SUBJECT, CLOUD_CONNECTOR_RESOURCE_TYPE,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The collection route `CreateCloudConnector` is bound to, and the prefix of the instance
/// route the update and the read are bound to
/// (`smithy_query(query="operation",operation="UpdateCloudConnector")` -> PUT
/// `/cloud-connectors/{Identifier}`).
const COLLECTION: &str = "/cloud-connectors";

/// The `@length` maximum of `DisplayName`, in characters
/// (`smithy_query(query="operation",operation="UpdateCloudConnector")` -> `Name`
/// len 1..128).
const DISPLAY_NAME_MAX_LENGTH: usize = 128;

/// The `@length` maximum of `CloudConnectorDescription`, in characters (same call ->
/// `Description` len 1..256).
const DESCRIPTION_MAX_LENGTH: usize = 256;

/// The `@length` maximum of `CloudConnectorId`, in characters (same call -> `Identifier`
/// len 1..64), which the absent-identifier rows are built against.
const CLOUD_CONNECTOR_ID_MAX_LENGTH: usize = 64;

/// The wire error code the model's `@httpError` 404 shape is rendered under.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn string(&self, name: &str) -> Option<&str> {
        self.body
            .get(name)
            .filter(|value| !value.is_null())
            .and_then(Value::as_str)
    }

    /// The `arn` of the `lambda` endpoint the answer reports.
    fn lambda_arn(&self) -> Option<&str> {
        self.body.get("EndpointConfig")?.get("lambda")?.get("arn")?.as_str()
    }
}

/// A router holding one `iotmanagedintegrations` service over the engine the caller keeps a
/// handle to: the same assembly `core/router` performs, with one service mounted.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// [`emulator_over`] over a fresh engine.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
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

/// The members a create names, which the update then has to leave alone except where it
/// names them itself.
const CREATED_NAME: &str = "connector before the update";
const CREATED_DESCRIPTION: &str = "Description before the update";
const CREATED_LAMBDA_ARN: &str = "connector-handler";
const CREATED_ENDPOINT_TYPE: &str = "LAMBDA";

/// One row of the update table: which members the request names, and in which form.
struct Case {
    /// What an assertion calls this row.
    label: &'static str,
    /// The `Name` the update names, or `None` on a row that names none.
    name: Option<String>,
    /// The `Description` the update names, or `None` on a row that names none.
    description: Option<String>,
}

impl Case {
    /// The `UpdateCloudConnectorRequest` body this row sends: both members are optional,
    /// so only the ones the row names are present.
    fn body(&self) -> Value {
        let mut body = json!({});
        if let Some(name) = &self.name {
            body["Name"] = json!(name);
        }
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        body
    }

    /// What a read must report for `Name` after this row: the member the row named, or the
    /// value the create stored when it named none.
    fn expected_name(&self) -> &str {
        self.name.as_deref().unwrap_or(CREATED_NAME)
    }

    /// The same for `Description`.
    fn expected_description(&self) -> &str {
        self.description.as_deref().unwrap_or(CREATED_DESCRIPTION)
    }
}

/// The table the update rule is checked over: the rows differ along which of the two
/// updatable members the request names, and the length and punctuation of their values
/// inside the modelled bounds (`DisplayName` len 1..128 `^[A-Za-z0-9-_ ]+$`,
/// `CloudConnectorDescription` len 1..256 `^[0-9A-Za-z_\- ]+$`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "both-members",
            name: Some("Renamed C2C Connector".to_string()),
            description: Some("Lamps and switches from the partner cloud".to_string()),
        },
        Case {
            label: "name-only",
            name: Some("Name-only_update 9".to_string()),
            description: None,
        },
        Case {
            label: "description-only",
            name: None,
            description: Some("9_dash-and space".to_string()),
        },
        Case {
            label: "neither-member",
            name: None,
            description: None,
        },
        Case {
            label: "shortest-admitted-values",
            name: Some("c".to_string()),
            description: Some("d".to_string()),
        },
        Case {
            label: "longest-admitted-values",
            name: Some("n".repeat(DISPLAY_NAME_MAX_LENGTH)),
            description: Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
        },
    ]
}

/// Create one connector carrying every member of the create surface, and answer its `Id`.
async fn create(router: &MultiServiceRouter, label: &str) -> String {
    let created = call(
        router,
        "POST",
        COLLECTION,
        Some(json!({
            "Name": CREATED_NAME,
            "Description": CREATED_DESCRIPTION,
            "EndpointType": CREATED_ENDPOINT_TYPE,
            "EndpointConfig": { "lambda": { "arn": CREATED_LAMBDA_ARN } },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {label:?}: expected HTTP 201 from the create, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {label:?}: expected an Id member from the create, got {:?}",
                created.body
            )
        })
        .to_string()
}

/// RULE: the update writes the members it names onto the stored record and leaves the rest
/// of it alone, so the read that follows reports exactly that.
#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_no_others() {
    let router = emulator();

    for case in cases() {
        let id = create(&router, case.label).await;

        let updated = call(
            &router,
            "PUT",
            &format!("{COLLECTION}/{id}"),
            Some(case.body()),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from the update, got {} with body {:?}",
            case.label, updated.status, updated.body
        );
        assert_eq!(
            updated.body,
            Value::Null,
            "case {:?}: the update's modelled output has no members, got {:?}",
            case.label,
            updated.body
        );

        let read = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from the read, got {} with body {:?}",
            case.label, read.status, read.body
        );
        assert_eq!(
            read.string("Name"),
            Some(case.expected_name()),
            "case {:?}: expected Name {:?}, got {:?}",
            case.label,
            case.expected_name(),
            read.string("Name")
        );
        assert_eq!(
            read.string("Description"),
            Some(case.expected_description()),
            "case {:?}: expected Description {:?}, got {:?}",
            case.label,
            case.expected_description(),
            read.string("Description")
        );
        // The members the update cannot name are the rest of the record: an update of one
        // member must not drop or rewrite any of them.
        assert_eq!(
            read.string("Id"),
            Some(id.as_str()),
            "case {:?}: expected the created id {id:?}, got {:?}",
            case.label,
            read.string("Id")
        );
        assert_eq!(
            read.string("EndpointType"),
            Some(CREATED_ENDPOINT_TYPE),
            "case {:?}: expected EndpointType {CREATED_ENDPOINT_TYPE:?}, got {:?}",
            case.label,
            read.string("EndpointType")
        );
        assert_eq!(
            read.lambda_arn(),
            Some(CREATED_LAMBDA_ARN),
            "case {:?}: expected the stored lambda arn {CREATED_LAMBDA_ARN:?}, got {:?}",
            case.label,
            read.lambda_arn()
        );
    }
}

/// RULE (one record, not a second one): an update mutates the record in place, so the
/// store still holds exactly the connector the create made, and a second update applied on
/// top of the first is what the read reports.
#[tokio::test]
async fn an_update_mutates_the_one_stored_record_in_place() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let id = create(&router, "in-place").await;
    // What the record should hold after each row, tracked across the chain: a row that
    // names a member replaces it, a row that names none leaves the previous row's value.
    let mut expected_name = CREATED_NAME.to_string();
    let mut expected_description = CREATED_DESCRIPTION.to_string();

    for case in cases() {
        if let Some(name) = &case.name {
            expected_name = name.clone();
        }
        if let Some(description) = &case.description {
            expected_description = description.clone();
        }
        let updated = call(
            &router,
            "PUT",
            &format!("{COLLECTION}/{id}"),
            Some(case.body()),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from the update, got {} with body {:?}",
            case.label, updated.status, updated.body
        );
        assert_eq!(
            engine.state::<ServiceState>().lock().cloud_connectors.len(),
            1,
            "case {:?}: an update must not store a second connector",
            case.label
        );

        // Applying the SAME update again is the same record: the second call reports what
        // the first one left behind.
        let again = call(
            &router,
            "PUT",
            &format!("{COLLECTION}/{id}"),
            Some(case.body()),
        )
        .await;
        assert_eq!(
            again.status, 200,
            "case {:?}: a repeated update is still accepted, got {} with body {:?}",
            case.label, again.status, again.body
        );
        let read = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            read.string("Name"),
            Some(expected_name.as_str()),
            "case {:?}: expected Name {expected_name:?}, got {:?}",
            case.label,
            read.string("Name")
        );
        assert_eq!(
            read.string("Description"),
            Some(expected_description.as_str()),
            "case {:?}: expected Description {expected_description:?}, got {:?}",
            case.label,
            read.string("Description")
        );
    }
}

/// RULE: an identifier the signing region's store holds no connector under is the modelled
/// 404, whatever form the identifier takes and whatever made the record absent — never
/// created, or created through another region's endpoint — and the update changes nothing.
#[tokio::test]
async fn an_update_of_a_connector_this_region_does_not_hold_is_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Identifiers inside the modelled `CloudConnectorId` constraints that no create
    // stored: a plain one, the shortest admitted, one at the `@length` maximum, and one
    // using the punctuation the `@pattern` admits.
    let absent = [
        "no-such-connector".to_string(),
        "x".to_string(),
        "x".repeat(CLOUD_CONNECTOR_ID_MAX_LENGTH),
        "Connector_9-ID".to_string(),
    ];
    for identifier in &absent {
        let refused = call(
            &router,
            "PUT",
            &format!("{COLLECTION}/{identifier}"),
            Some(json!({ "Name": "a name nothing takes" })),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {identifier:?}: expected HTTP 404, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(RESOURCE_NOT_FOUND),
            "case {identifier:?}: expected the modelled 404, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.string("ResourceId"),
            Some(identifier.as_str()),
            "case {identifier:?}: the 404 reports the identifier the request carried, \
             got {:?}",
            refused.string("ResourceId")
        );
        assert_eq!(
            refused.string("ResourceType"),
            Some(CLOUD_CONNECTOR_RESOURCE_TYPE),
            "case {identifier:?}: expected ResourceType {CLOUD_CONNECTOR_RESOURCE_TYPE:?}, \
             got {:?}",
            refused.string("ResourceType")
        );
        // The reason is the ONE sentence this resource's 404s carry, read and mutation
        // alike, and it names no identifier, so it is the same for every row.
        assert_eq!(
            refused.string("Message"),
            Some(format!("{CLOUD_CONNECTOR_RESOURCE_SUBJECT} not found.").as_str()),
            "case {identifier:?}: expected this resource's absent-connector reason, got {:?}",
            refused.string("Message")
        );
        assert!(
            engine
                .state::<ServiceState>()
                .lock()
                .cloud_connectors
                .is_empty(),
            "case {identifier:?}: a refused update must store nothing"
        );
    }

    // The same identifier, through another region's endpoint: a connector created in one
    // region is not updated by another, and its own region still reports the old values.
    let id = create(&router, "regional").await;
    for region in ["us-west-2", "eu-central-1"] {
        let elsewhere = call_signed(
            &router,
            region,
            "PUT",
            &format!("{COLLECTION}/{id}"),
            Some(json!({ "Name": "renamed from elsewhere" })),
        )
        .await;
        assert_eq!(
            elsewhere.status, 404,
            "case {region:?}: a connector of another region must not be updated, got {} \
             with body {:?}",
            elsewhere.status, elsewhere.body
        );
    }
    let home = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
    assert_eq!(
        home.string("Name"),
        Some(CREATED_NAME),
        "case \"regional\": the refused updates must have changed nothing, got {:?}",
        home.string("Name")
    );
}

/// RULE: an update addresses ONE connector — the record the `Identifier` names — so the
/// other connectors of the same region are untouched by it.
#[tokio::test]
async fn an_update_touches_only_the_connector_it_names() {
    let router = emulator();
    let target = create(&router, "target").await;
    let bystander = create(&router, "bystander").await;

    let updated = call(
        &router,
        "PUT",
        &format!("{COLLECTION}/{target}"),
        Some(json!({ "Name": "only the target", "Description": "only the target too" })),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case \"target\": expected HTTP 200, got {} with body {:?}",
        updated.status, updated.body
    );

    let read = call(&router, "GET", &format!("{COLLECTION}/{bystander}"), None).await;
    assert_eq!(
        read.string("Name"),
        Some(CREATED_NAME),
        "case \"bystander\": expected the untouched Name {CREATED_NAME:?}, got {:?}",
        read.string("Name")
    );
    assert_eq!(
        read.string("Description"),
        Some(CREATED_DESCRIPTION),
        "case \"bystander\": expected the untouched Description \
         {CREATED_DESCRIPTION:?}, got {:?}",
        read.string("Description")
    );
}
