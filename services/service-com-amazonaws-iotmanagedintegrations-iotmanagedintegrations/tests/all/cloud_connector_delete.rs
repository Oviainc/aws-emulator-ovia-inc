// SPDX-License-Identifier: CC0-1.0

//! `DeleteCloudConnector` over the wire: `CreateCloudConnector` -> `DeleteCloudConnector`
//! -> `GetCloudConnector`.
//!
//! RULE: `DeleteCloudConnector` (DELETE `/cloud-connectors/{Identifier}`, success 200 with
//! an empty body) REMOVES the cloud-connector record the signing region's store holds under
//! that identifier, so a following read of it reports it absent while every other stored
//! connector is left exactly as it was, and the `ClientToken` that created it stops
//! replaying into the removed id; an identifier that region holds no connector under —
//! never created, already deleted, or created through another region's endpoint — removes
//! nothing and is the modelled `ResourceNotFoundException` (HTTP 404), reporting the
//! identifier the request carried on `ResourceId`, the resource kind on `ResourceType`, and
//! the sentence the corpus records for an absent cloud connector on both halves of this
//! resource's surface — the same text its recorded read and mutating 404s carry — which
//! names no identifier and so is the same for every request.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.cloud-connector-lifecycle`) is ONE row of the tables below.
//! The rows differ along what the rule generalises over: which optional members the deleted
//! connector was created with and in which form (the `@pattern`/`@length` of `Name` and
//! `Description` at both bounds, the forms of Lambda ARN `LambdaArn` admits), how many
//! connectors the store holds when one of them is deleted, the form of an absent identifier
//! inside `CloudConnectorId`'s `@length` 1..64 and `^[A-Za-z0-9-_]+$`, what made it absent,
//! and the region the request is signed for.
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

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), which
/// the full-form Lambda ARN below is built for.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The collection route `CreateCloudConnector` is bound to, and the prefix of the instance
/// route the delete and the read are bound to
/// (`smithy_query(query="operation",operation="DeleteCloudConnector")` -> DELETE
/// `/cloud-connectors/{Identifier}`).
const COLLECTION: &str = "/cloud-connectors";

/// The modelled success status of `DeleteCloudConnector` (same call -> `http` code 200).
const DELETE_STATUS: u16 = 200;

/// The wire error code the model's `@httpError` 404 shape is rendered under.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `@length` maximum of `DisplayName`, in characters
/// (`smithy_query(query="operation",operation="CreateCloudConnector")` -> `Name`
/// len 1..128).
const DISPLAY_NAME_MAX_LENGTH: usize = 128;

/// The `@length` maximum of `CloudConnectorDescription`, in characters (same call ->
/// `Description` len 1..256).
const DESCRIPTION_MAX_LENGTH: usize = 256;

/// The `@length` bounds of `CloudConnectorId`, in characters
/// (`smithy_query(query="operation",operation="DeleteCloudConnector")` -> `Identifier`
/// len 1..64), which the absent-identifier rows are built against.
const CLOUD_CONNECTOR_ID_LENGTH: (usize, usize) = (1, 64);

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

/// [`call`], signed for an explicit region, so a regional resource can be addressed through
/// another region's endpoint.
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

/// One row of the delete table: the connector that gets created and then removed.
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
    /// The `CreateCloudConnector` body this row sends: `Name` and `EndpointConfig` are the
    /// model's required members, the rest are present only where the row names them.
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

/// The table the delete rule is checked over: the rows differ along which optional members
/// the removed connector carried and in which form — the shortest and the longest `Name` and
/// `Description` their `@length` bounds admit, the punctuation their `@pattern`s admit, and
/// the forms of Lambda ARN `LambdaArn` admits (bare name, qualified, full ARN).
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "required-members-only",
            name: "partner cloud connector".to_string(),
            description: None,
            endpoint_type: None,
            lambda_arn: "connector-handler".to_string(),
        },
        Case {
            label: "every-member-full-arn",
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

/// Create one connector through the wire and answer the `Id` the service minted for it.
async fn create(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let created = call(router, "POST", COLLECTION, Some(body)).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateCloudConnector, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an Id member, got {:?}",
                created.body
            )
        })
        .to_string()
}

/// Assert one answer is the modelled 404 of an absent cloud connector.
fn assert_not_found(case: &str, reply: &Reply, identifier: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(RESOURCE_NOT_FOUND),
        "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND}, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("ResourceId"),
        Some(identifier),
        "case {case:?}: the 404 reports the identifier the request carried, got {:?}",
        reply.string("ResourceId")
    );
    assert_eq!(
        reply.string("ResourceType"),
        Some(CLOUD_CONNECTOR_RESOURCE_TYPE),
        "case {case:?}: expected ResourceType {CLOUD_CONNECTOR_RESOURCE_TYPE:?}, got {:?}",
        reply.string("ResourceType")
    );
    // The sentence is about the resource and names no identifier, so it is the same for
    // every row; the identifier is reported on `ResourceId` instead.
    let expected = format!("{CLOUD_CONNECTOR_RESOURCE_SUBJECT} not found.");
    assert_eq!(
        reply.string("Message"),
        Some(expected.as_str()),
        "case {case:?}: expected the reason {expected:?}, got {:?}",
        reply.string("Message")
    );
}

/// RULE (round trip): a delete removes the stored record — the call answers the modelled
/// success with no body and no error member, the read that reported the connector a moment
/// earlier now reports it absent, and the store holds it no longer.
#[tokio::test]
async fn a_delete_removes_the_connector_a_read_reported() {
    for case in cases() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_over(Arc::clone(&engine));
        let id = create(&router, case.label, case.body()).await;

        let read = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the created connector must read back before the delete, got {} \
             with body {:?}",
            case.label, read.status, read.body
        );

        let deleted = call(&router, "DELETE", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            deleted.status, DELETE_STATUS,
            "case {:?}: expected HTTP {DELETE_STATUS}, got {} with body {:?}",
            case.label, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_type, None,
            "case {:?}: a successful delete carries no error code, got {:?}",
            case.label, deleted.error_type
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {:?}: the modelled output has no members, so the body is empty, got {:?}",
            case.label,
            deleted.body
        );

        let gone = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_not_found(case.label, &gone, &id);
        assert!(
            engine
                .state::<ServiceState>()
                .lock()
                .cloud_connectors
                .is_empty(),
            "case {:?}: the removed record must be gone from the store",
            case.label
        );
        assert!(
            !engine
                .state::<ServiceState>()
                .lock()
                .cloud_connector_order
                .contains(&id),
            "case {:?}: the removed id must be gone from the insertion order",
            case.label
        );
    }
}

/// RULE (isolation): a delete removes ONE record — every other connector the store holds
/// still reads back unchanged, whatever the order the deletes are performed in.
#[tokio::test]
async fn a_delete_leaves_every_other_connector_where_it_was() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // The whole table in one store, so each delete has neighbours on both sides of it.
    let table = cases();
    let mut ids = Vec::new();
    for case in &table {
        ids.push(create(&router, case.label, case.body()).await);
    }

    // Delete from the middle outwards, so no row is removed from an end of the insertion
    // order every time. Derived from the table, so a row added to it is covered too.
    let middle = table.len() / 2;
    let mut order: Vec<usize> = (0..table.len()).collect();
    order.sort_by_key(|at| (at.abs_diff(middle), *at));
    let mut live: Vec<usize> = (0..table.len()).collect();
    for removed in order {
        let case = table[removed].label;
        let deleted = call(
            &router,
            "DELETE",
            &format!("{COLLECTION}/{}", ids[removed]),
            None,
        )
        .await;
        assert_eq!(
            deleted.status, DELETE_STATUS,
            "case {case:?}: expected HTTP {DELETE_STATUS}, got {} with body {:?}",
            deleted.status, deleted.body
        );
        live.retain(|at| *at != removed);

        for at in &live {
            let still = call(&router, "GET", &format!("{COLLECTION}/{}", ids[*at]), None).await;
            assert_eq!(
                still.status, 200,
                "case {case:?}: deleting it must leave {:?} readable, got {} with body {:?}",
                table[*at].label, still.status, still.body
            );
            assert_eq!(
                still.string("Name"),
                Some(table[*at].name.as_str()),
                "case {case:?}: {:?} must still report its own Name, got {:?}",
                table[*at].label,
                still.string("Name")
            );
        }
        assert_eq!(
            engine.state::<ServiceState>().lock().cloud_connectors.len(),
            live.len(),
            "case {case:?}: the store must hold exactly the connectors not yet deleted"
        );
    }
}

/// RULE: an identifier the signing region's store holds no connector under removes nothing
/// and is the modelled 404 — whatever form the identifier takes and whatever made the record
/// absent: never created, already deleted, or created through another region's endpoint.
#[tokio::test]
async fn a_delete_of_a_connector_this_region_does_not_hold_is_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let (min_length, max_length) = CLOUD_CONNECTOR_ID_LENGTH;

    // Identifiers inside the modelled `CloudConnectorId` constraints that no create stored:
    // a plain one, both ends of the `@length` bound, the punctuation the `@pattern` admits,
    // digits only, and mixed case.
    let never_created = [
        ("plain", "no-such-connector".to_string()),
        ("shortest admitted", "x".repeat(min_length)),
        ("longest admitted", "x".repeat(max_length)),
        ("the punctuation the pattern admits", "Connector_9-ID".to_string()),
        ("digits only", "0123456789".to_string()),
        ("mixed case", "MiXeDcAsE".to_string()),
    ];
    for (form, identifier) in &never_created {
        let refused = call(&router, "DELETE", &format!("{COLLECTION}/{identifier}"), None).await;
        assert_not_found(form, &refused, identifier);
    }

    // One connector that EXISTS, so the rows below remove something real and the store is
    // observed before and after each refusal.
    let case = &cases()[1];
    let id = create(&router, case.label, case.body()).await;

    // Another region's endpoint holds no record under this id, so it deletes nothing and
    // the creating region still reports the connector.
    for region in ["us-west-2", "eu-central-1"] {
        let elsewhere = call_signed(
            &router,
            region,
            "DELETE",
            &format!("{COLLECTION}/{id}"),
            None,
        )
        .await;
        assert_not_found(region, &elsewhere, &id);
        let untouched = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            untouched.status, 200,
            "case {region:?}: another region's delete must leave the record where it is, \
             got {} with body {:?}",
            untouched.status, untouched.body
        );
        assert_eq!(
            engine.state::<ServiceState>().lock().cloud_connectors.len(),
            1,
            "case {region:?}: another region's delete must remove nothing"
        );
    }

    // The same identifier, after the connector has really been removed: a second delete
    // finds nothing to remove and states it exactly as a never-created identifier does.
    let deleted = call(&router, "DELETE", &format!("{COLLECTION}/{id}"), None).await;
    assert_eq!(
        deleted.status, DELETE_STATUS,
        "case \"already deleted\": the first delete must succeed, got {} with body {:?}",
        deleted.status, deleted.body
    );
    let again = call(&router, "DELETE", &format!("{COLLECTION}/{id}"), None).await;
    assert_not_found("already deleted", &again, &id);
    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .cloud_connectors
            .is_empty(),
        "case \"already deleted\": a refused delete must leave the store as it was"
    );
}

/// RULE: the `ClientToken` that created a connector stops replaying into it once the
/// connector is removed — a create repeating that token mints a NEW connector rather than
/// answering with the deleted id, which no longer resolves.
#[tokio::test]
async fn a_client_token_stops_replaying_into_a_deleted_connector() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // The forms the modelled `ClientToken` `@pattern` (`^[a-zA-Z0-9=_-]+$`, len 1..64)
    // admits: a hyphenated token, an upper-case padded one, and a single character.
    for token in ["retry-token-1", "TOKEN_2==", "3"] {
        let body = json!({
            "Name": "token connector",
            "EndpointConfig": { "lambda": { "arn": "connector-handler" } },
            "ClientToken": token,
        });
        let first = create(&router, token, body.clone()).await;

        // While the connector is there, the token replays into it.
        let replayed = create(&router, token, body.clone()).await;
        assert_eq!(
            replayed, first,
            "case {token:?}: a replayed token must answer with {first:?}, got {replayed:?}"
        );

        let deleted = call(&router, "DELETE", &format!("{COLLECTION}/{first}"), None).await;
        assert_eq!(
            deleted.status, DELETE_STATUS,
            "case {token:?}: expected HTTP {DELETE_STATUS}, got {} with body {:?}",
            deleted.status, deleted.body
        );

        // After the delete the same token creates a new connector instead of handing back
        // an id that resolves to nothing.
        let after = create(&router, token, body).await;
        assert_ne!(
            after, first,
            "case {token:?}: the token must not replay into the deleted id {first:?}"
        );
        let read = call(&router, "GET", &format!("{COLLECTION}/{after}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {token:?}: the newly created connector must read back, got {} with body {:?}",
            read.status, read.body
        );
        let gone = call(&router, "GET", &format!("{COLLECTION}/{first}"), None).await;
        assert_not_found(token, &gone, &first);

        // Leave the store empty for the next row.
        let cleanup = call(&router, "DELETE", &format!("{COLLECTION}/{after}"), None).await;
        assert_eq!(
            cleanup.status, DELETE_STATUS,
            "case {token:?}: expected HTTP {DELETE_STATUS}, got {} with body {:?}",
            cleanup.status, cleanup.body
        );
        assert!(
            engine
                .state::<ServiceState>()
                .lock()
                .cloud_connectors
                .is_empty(),
            "case {token:?}: both connectors of this row must be gone"
        );
    }
}
