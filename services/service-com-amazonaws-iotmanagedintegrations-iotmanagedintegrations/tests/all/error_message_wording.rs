// SPDX-License-Identifier: CC0-1.0

//! The WORDING of this service's refusals, over the wire.
//!
//! RULE: the `Message` of a modelled error is part of the error contract, and the sentence
//! it carries belongs to the resource and the path that answered — not to a template over
//! every resource. So:
//!
//! * a read that resolves a resource and finds none states that resource's OWN sentence,
//!   byte for byte, and fills only the `ResourceNotFoundException` members that resource's
//!   backend fills: `Account association not found.` and `Cloud connector not found.` beside
//!   `ResourceId`/`ResourceType`, `Event log configuration doesn't exist` with neither of
//!   them, and `OTA task configuration not found for id: <the Identifier sent>` — the one
//!   sentence of the four that interpolates a request value, and nothing else;
//! * a create that RESOLVES a reference to another resource states the REFERENCED
//!   resource's absence (`Destination doesn't exist` for a `DestinationName` this region
//!   has no destination under), quoting no name back;
//! * a create judges what the request CARRIES before it asks the store about anything, so a
//!   `CreateConnectorDestination` with no Secrets Manager ARN is answered
//!   `SecretsManagerArn cannot be null or empty` even when its `CloudConnectorId` also names
//!   no connector — the caller is told about the field it left out, not about the resource
//!   the store would have been asked for.
//!
//! A sentence that quotes no request value is the same for every request that reaches it,
//! whatever made the resource absent (never created, deleted, or another region's) and
//! whatever form the identifier took.
//!
//! The recorded units this was derived from are ONE row of each table below
//! (`iot-managed-integrations-account-association-error-paths#task#1`,
//! `-cloud-connector-error-paths#task#2`,
//! `-event-log-configuration-wildcard-lifecycle#task#2`,
//! `-ota-task-configuration-lifecycle#task#2`,
//! `-notification-configuration-error-paths#task#4` and
//! `-connector-destination-error-paths#task#4`). The rows differ along what the rule
//! generalises over: the resource whose absence is stated, the form and length of the
//! identifier inside each member's modelled `@length`/`@pattern`, the CAUSE of the absence,
//! the operation that resolves the reference, and which members a refused create carries.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the statuses, headers and bodies asserted here are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::{
    DELIVERY_DESTINATION_TYPE_VALUES, EVENT_TYPE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// Another region's endpoint, for the cases that address one region's resource through
/// another's.
const OTHER_REGION: &str = "eu-central-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), which
/// the Secrets Manager ARNs below are built for (the `SecretsManagerArn` `@pattern` requires
/// 12 digits there).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The wire error codes the model's `@httpError` shapes are rendered under.
const NOT_FOUND: &str = "ResourceNotFoundException";
const VALIDATION: &str = "ValidationException";

/// The members the modelled `ResourceNotFoundException` declares, all optional
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` -> `Message`,
/// `ResourceId`, `ResourceType`, `httpError` 404).
const RESOURCE_MEMBERS: [&str; 2] = ["ResourceId", "ResourceType"];

/// The sentences the rule requires, written out here rather than imported so the suite pins
/// the text a client reads and not the implementation's own formatter.
const ACCOUNT_ASSOCIATION_REASON: &str = "Account association not found.";
const CLOUD_CONNECTOR_REASON: &str = "Cloud connector not found.";
const EVENT_LOG_CONFIGURATION_REASON: &str = "Event log configuration doesn't exist";
/// The one read sentence of the four that interpolates a request value: this prefix, then
/// the `Identifier` the request carried.
const OTA_TASK_CONFIGURATION_REASON_PREFIX: &str = "OTA task configuration not found for id: ";
/// The absent destination a notification configuration was pointed at.
const DESTINATION_REASON: &str = "Destination doesn't exist";
/// The field a create of a connector destination may not leave out.
const SECRETS_MANAGER_ARN_REASON: &str = "SecretsManagerArn cannot be null or empty";
/// The sentence the connector REFERENCE of that same create states, which this suite drives
/// only to show that it is reached once the field above is supplied.
const CLOUD_CONNECTOR_REFERENCE_REASON: &str = "The provided CloudConnectorId does not exist.";

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

/// A router holding one `iotmanagedintegrations` service over `engine`: the same assembly
/// `core/router` performs, with one service mounted.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// [`emulator_over`] over an engine the test does not keep a handle to.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path}"
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

/// [`call_in`] in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    call_in(router, REGION, method, path, body).await
}

/// How a resource's read-path sentence is built: fixed text, or fixed text followed by the
/// identifier the request carried.
#[derive(Debug, Clone, Copy)]
enum Reason {
    /// The same sentence for every request, whatever identifier it carried.
    Fixed(&'static str),
    /// This prefix, then the identifier the request carried.
    QuotingTheIdentifier(&'static str),
}

impl Reason {
    fn expected(self, identifier: &str) -> String {
        match self {
            Self::Fixed(text) => text.to_string(),
            Self::QuotingTheIdentifier(prefix) => format!("{prefix}{identifier}"),
        }
    }
}

/// One row of the read-path table: a resource, the read that resolves it, the sentence its
/// backend states for an absent one, the `ResourceType` its 404 reports (`None` where that
/// resource's 404 carries neither `ResourceId` nor `ResourceType`), and identifiers inside
/// the modelled constraints of the member the read takes.
struct AbsentResource {
    label: &'static str,
    /// The collection the read's `@http` uri names
    /// (`smithy_query(query="operation",operation="GetAccountAssociation,GetCloudConnector,\
    /// GetEventLogConfiguration,GetOtaTaskConfiguration")`).
    collection: &'static str,
    reason: Reason,
    resource_type: Option<&'static str>,
    identifiers: Vec<(&'static str, String)>,
}

/// The read-path table. Every row's identifiers stay inside the `@length`/`@pattern` of the
/// member that read takes, and differ along form, length (both modelled bounds), character
/// class and case, so the sentence is checked over the class of identifiers the model admits
/// rather than over the one the recording carried.
fn absent_resources() -> Vec<AbsentResource> {
    vec![
        // `AccountAssociationId`: len 1..64, `^[0-9a-zA-Z]+$`.
        AbsentResource {
            label: "GetAccountAssociation",
            collection: "/account-associations",
            reason: Reason::Fixed(ACCOUNT_ASSOCIATION_REASON),
            resource_type: Some("AccountAssociation"),
            identifiers: vec![
                ("plain", "nosuchaccountassociation".to_string()),
                ("shortest", "z".to_string()),
                ("longest", "z".repeat(64)),
                ("digits only", "0123456789".to_string()),
                ("mixed case", "MiXeDcAsE".to_string()),
            ],
        },
        // `CloudConnectorId`: len 1..64, `^[A-Za-z0-9-_]+$`.
        AbsentResource {
            label: "GetCloudConnector",
            collection: "/cloud-connectors",
            reason: Reason::Fixed(CLOUD_CONNECTOR_REASON),
            resource_type: Some("CloudConnector"),
            identifiers: vec![
                ("plain", "no-such-connector".to_string()),
                ("shortest", "x".to_string()),
                ("longest", "x".repeat(64)),
                ("the punctuation the pattern admits", "Zed-9_id".to_string()),
                ("digits only", "9876543210".to_string()),
            ],
        },
        // `LogConfigurationId`: len 1..200, `^[A-Za-z0-9]+$`.
        AbsentResource {
            label: "GetEventLogConfiguration",
            collection: "/event-log-configurations",
            reason: Reason::Fixed(EVENT_LOG_CONFIGURATION_REASON),
            resource_type: None,
            identifiers: vec![
                ("plain", "nosuchlogconfiguration".to_string()),
                ("shortest", "z".to_string()),
                ("longest", "z".repeat(200)),
                ("digits only", "0071".to_string()),
                ("mixed case", "MiXeDcAsE".to_string()),
            ],
        },
        // `OtaTaskConfigurationId`: len 1..64, `^[a-zA-Z0-9]*$`.
        AbsentResource {
            label: "GetOtaTaskConfiguration",
            collection: "/ota-task-configurations",
            reason: Reason::QuotingTheIdentifier(OTA_TASK_CONFIGURATION_REASON_PREFIX),
            resource_type: None,
            identifiers: vec![
                ("plain", "nosuchtaskconfiguration".to_string()),
                ("shortest", "z".to_string()),
                ("longest", "z".repeat(64)),
                ("digits only", "0123456789".to_string()),
                ("mixed case", "MiXeDcAsE".to_string()),
            ],
        },
    ]
}

/// Assert one 404 against the row's sentence and the members that resource's 404 carries.
fn assert_not_found(case: &str, reply: &Reply, row: &AbsentResource, identifier: &str) {
    let expected = row.reason.expected(identifier);
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(NOT_FOUND),
        "case {case:?}: expected the modelled {NOT_FOUND} code, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("Message"),
        Some(expected.as_str()),
        "case {case:?}: expected the reason {expected:?}, got {:?}",
        reply.string("Message")
    );
    match row.resource_type {
        Some(resource_type) => {
            assert_eq!(
                reply.string("ResourceId"),
                Some(identifier),
                "case {case:?}: expected ResourceId {identifier:?}, got {:?}",
                reply.string("ResourceId")
            );
            assert_eq!(
                reply.string("ResourceType"),
                Some(resource_type),
                "case {case:?}: expected ResourceType {resource_type:?}, got {:?}",
                reply.string("ResourceType")
            );
        }
        None => {
            for member in RESOURCE_MEMBERS {
                assert!(
                    reply.member(member).is_none(),
                    "case {case:?}: this resource's 404 carries no {member}, got {:?}",
                    reply.member(member)
                );
            }
        }
    }
}

/// RULE (see the module docs): each resource's read states its OWN sentence, over the class
/// of identifiers its member admits — and a sentence that quotes no identifier never echoes
/// one, while the one that does reproduces it exactly.
#[tokio::test]
async fn each_read_states_its_own_resources_sentence() {
    let router = emulator();

    for row in absent_resources() {
        let mut reasons: BTreeSet<String> = BTreeSet::new();
        for (form, identifier) in &row.identifiers {
            let case = format!("{}/{form}", row.label);
            let reply = call(
                &router,
                "GET",
                &format!("{}/{identifier}", row.collection),
                None,
            )
            .await;
            assert_not_found(&case, &reply, &row, identifier);

            let reason = reply.string("Message").unwrap_or_default().to_string();
            match row.reason {
                Reason::Fixed(_) => assert!(
                    !reason.contains(identifier.as_str()),
                    "case {case:?}: a fixed sentence must not echo the identifier, got \
                     {reason:?}"
                ),
                Reason::QuotingTheIdentifier(prefix) => assert!(
                    reason.strip_prefix(prefix) == Some(identifier.as_str()),
                    "case {case:?}: the sentence must quote the identifier as the request \
                     spelled it, got {reason:?}"
                ),
            }
            reasons.insert(reason);
        }

        // A fixed sentence is ONE sentence for the whole row; the identifier-quoting one is
        // a different sentence per identifier.
        let expected_distinct = match row.reason {
            Reason::Fixed(_) => 1,
            Reason::QuotingTheIdentifier(_) => row.identifiers.len(),
        };
        assert_eq!(
            reasons.len(),
            expected_distinct,
            "case {:?}: expected {expected_distinct} distinct sentence(s) over {} \
             identifiers, got {reasons:?}",
            row.label,
            row.identifiers.len()
        );
    }
}

/// `CreateCloudConnector` in `region`, answering the `CloudConnectorId` it minted.
async fn cloud_connector(router: &MultiServiceRouter, region: &str) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        "/cloud-connectors",
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

/// RULE: the sentence does not depend on WHY the resource is absent. A configuration that
/// was created and deleted is stated with the same sentence as one that never existed, and a
/// connector that exists only in another region with the same sentence as one this region
/// never held — both under identifiers the SERVICE minted rather than ones the test chose.
#[tokio::test]
async fn the_cause_of_the_absence_does_not_change_the_sentence() {
    let router = emulator();

    // Created and then deleted: the event log configuration, whose delete this emulator
    // serves.
    let event_log = AbsentResource {
        label: "GetEventLogConfiguration",
        collection: "/event-log-configurations",
        reason: Reason::Fixed(EVENT_LOG_CONFIGURATION_REASON),
        resource_type: None,
        identifiers: Vec::new(),
    };
    let created = call(
        &router,
        "POST",
        event_log.collection,
        // The account wildcard target, which `SmartHomeResourceType` admits
        // (`smithy_query(query="shape",shape="SmartHomeResourceType")` -> `^[*]$|^(managed-
        // thing|credential-locker|provisioning-profile|ota-task|account-association)$`).
        Some(json!({ "ResourceType": "*", "EventLogLevel": "INFO" })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding an event log configuration: expected HTTP 201, got {} with body {:?}",
        created.status, created.body
    );
    let deleted = created
        .string("Id")
        .expect("the create answers with an Id")
        .to_string();
    let removed = call(
        &router,
        "DELETE",
        &format!("{}/{deleted}", event_log.collection),
        None,
    )
    .await;
    assert!(
        (200..300).contains(&removed.status),
        "seeding \"deleted\": expected the delete to succeed, got {} with body {:?}",
        removed.status,
        removed.body
    );
    let gone = call(
        &router,
        "GET",
        &format!("{}/{deleted}", event_log.collection),
        None,
    )
    .await;
    assert_not_found(
        "GetEventLogConfiguration/deleted",
        &gone,
        &event_log,
        &deleted,
    );

    // Alive, but in another region: this region's endpoint holds none under that id.
    let connector = AbsentResource {
        label: "GetCloudConnector",
        collection: "/cloud-connectors",
        reason: Reason::Fixed(CLOUD_CONNECTOR_REASON),
        resource_type: Some("CloudConnector"),
        identifiers: Vec::new(),
    };
    let elsewhere = cloud_connector(&router, OTHER_REGION).await;
    let here = call(
        &router,
        "GET",
        &format!("{}/{elsewhere}", connector.collection),
        None,
    )
    .await;
    assert_not_found(
        "GetCloudConnector/another region",
        &here,
        &connector,
        &elsewhere,
    );
}

/// A `CreateDestination` body for `name` in `region`: the delivery target a notification
/// configuration is pointed at.
fn destination_request(name: &str, region: &str) -> Value {
    json!({
        "Name": name,
        "DeliveryDestinationArn":
            format!("arn:aws:kinesis:{region}:{ACCOUNT}:stream/notifications"),
        "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/managed-integrations"),
    })
}

/// Create the destination `name` in `region`, asserting the modelled 201.
async fn destination(router: &MultiServiceRouter, region: &str, name: &str) {
    let created = call_in(
        router,
        region,
        "POST",
        "/destinations",
        Some(destination_request(name, region)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding the destination {name:?} in {region:?}: expected HTTP 201, got {} with \
         body {:?}",
        created.status, created.body
    );
}

/// RULE: a notification configuration pointed at a `DestinationName` this region has no
/// destination under is refused with the REFERENCED resource's sentence — the same from the
/// create and from the update, for every form of name and every cause of the absence, and it
/// quotes no name back.
#[tokio::test]
async fn an_absent_destination_is_stated_by_the_destination_it_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // A destination that exists, to point the configuration the UPDATE cases change at, and
    // one that exists only in ANOTHER region.
    destination(&router, REGION, "live destination").await;
    destination(&router, OTHER_REGION, "out of region").await;

    // `DestinationName`: len 1..128, `^[\p{L}\p{N} ._-]+$`. The rows differ along the form
    // and length of the name and along why it resolves to nothing: never created, another
    // region's, and one differing from a live destination only in case or spacing.
    let absent = [
        ("plain", "no such destination".to_string()),
        // The one-character row is spelled `q`, a letter the sentence does not contain, so
        // the "quotes no name back" assertion below is meaningful for it too.
        ("shortest", "q".to_string()),
        ("longest", "d".repeat(128)),
        ("the punctuation the pattern admits", "Dots.and_dashes-1".to_string()),
        ("digits only", "20260101".to_string()),
        ("another region's", "out of region".to_string()),
        ("differing only in case", "Live Destination".to_string()),
        ("differing only in spacing", "live  destination".to_string()),
    ];

    for (index, (form, name)) in absent.iter().enumerate() {
        // Each row takes its own `EventType` from the model's value set, so no row is
        // refused by the one-configuration-per-event-type conflict instead.
        let event_type = EVENT_TYPE_VALUES[index % EVENT_TYPE_VALUES.len()];

        let refused = call(
            &router,
            "POST",
            "/notification-configurations",
            Some(json!({ "EventType": event_type, "DestinationName": name })),
        )
        .await;
        assert_refused_with(
            &format!("CreateNotificationConfiguration/{form}"),
            &refused,
            DESTINATION_REASON,
        );
        assert!(
            !refused.string("Message").unwrap_or_default().contains(name.as_str()),
            "case {form:?}: the refusal must not quote the name back, got {:?}",
            refused.string("Message")
        );

        // The create stored nothing, so the configuration is still absent.
        let read = call(
            &router,
            "GET",
            &format!("/notification-configurations/{event_type}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {form:?}: a refused create must store nothing, got {} with body {:?}",
            read.status, read.body
        );
    }

    // The same reference, resolved by the UPDATE of a configuration that DOES exist: one
    // sentence for the resource, whichever operation resolved the name.
    let live_event_type = EVENT_TYPE_VALUES[0];
    let created = call(
        &router,
        "POST",
        "/notification-configurations",
        Some(json!({
            "EventType": live_event_type,
            "DestinationName": "live destination",
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding a configuration for {live_event_type:?}: expected HTTP 201, got {} with \
         body {:?}",
        created.status, created.body
    );
    for (form, name) in &absent {
        let refused = call(
            &router,
            "PUT",
            &format!("/notification-configurations/{live_event_type}"),
            Some(json!({ "DestinationName": name })),
        )
        .await;
        assert_refused_with(
            &format!("UpdateNotificationConfiguration/{form}"),
            &refused,
            DESTINATION_REASON,
        );
    }
    // A refused update left the configuration pointed where it was.
    let read = call(
        &router,
        "GET",
        &format!("/notification-configurations/{live_event_type}"),
        None,
    )
    .await;
    assert_eq!(
        read.string("DestinationName"),
        Some("live destination"),
        "a refused update must leave the stored destination as it was, got {:?}",
        read.body
    );
}

/// Assert one answer is the modelled `ValidationException` carrying exactly `reason`.
fn assert_refused_with(case: &str, reply: &Reply, reason: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(VALIDATION),
        "case {case:?}: expected the modelled {VALIDATION} code, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("Message"),
        Some(reason),
        "case {case:?}: expected the reason {reason:?}, got {:?}",
        reply.string("Message")
    );
}

/// One `SecretsManager` configuration, inside the `SecretsManagerArn` `@pattern`
/// (`^arn:aws:secretsmanager:[0-9a-zA-Z-]{1,32}:\d{12}:secret:[A-Za-z0-9/_+=.@-]{8,520}$`)
/// and the `SecretsManagerVersionId` `@length` (32..64).
fn secret(name: &str) -> Value {
    json!({
        "arn": format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{name}-A1b2C3"),
        "versionId": "a".repeat(32),
    })
}

/// One row of the connector-destination table: what an assertion calls it, and the body it
/// sends apart from `CloudConnectorId` and the Secrets Manager configuration the cases add.
struct DestinationBody {
    label: &'static str,
    members: Value,
}

/// The rows the Secrets Manager requirement is checked over: they differ along which
/// optional members the create names and which authentication configuration its `@required`
/// `AuthConfig` carries, so the refusal is about the ARN that is missing and not about the
/// rest of the request.
fn destination_bodies() -> Vec<DestinationBody> {
    vec![
        DestinationBody {
            label: "required members only",
            members: json!({ "AuthConfig": {} }),
        },
        DestinationBody {
            label: "oauth",
            members: json!({
                "AuthType": "OAUTH",
                "Name": "Partner Destination",
                "Description": "Lamps and switches from the partner cloud",
                "AuthConfig": {
                    "oAuth": {
                        "authUrl": "https://partner.example.com/oauth2/authorize",
                        "tokenUrl": "https://partner.example.com/oauth2/token",
                        "tokenEndpointAuthenticationScheme": "HTTP_BASIC",
                    },
                },
            }),
        },
        DestinationBody {
            label: "general authorization",
            members: json!({
                "Name": "d",
                "AuthConfig": {
                    "GeneralAuthorization": [{
                        "SecretsManager": secret("material"),
                        "AuthMaterialName": "partner-key",
                    }],
                },
            }),
        },
        DestinationBody {
            label: "with a client token",
            members: json!({
                "AuthConfig": {},
                "ClientToken": "token-1",
            }),
        },
    ]
}

impl DestinationBody {
    /// This row's body against `connector`, with the top-level `SecretsManager` only where
    /// `secrets_manager` names one.
    fn body(&self, connector: &str, secrets_manager: Option<Value>) -> Value {
        let mut body = self.members.clone();
        body["CloudConnectorId"] = json!(connector);
        if let Some(secrets_manager) = secrets_manager {
            body["SecretsManager"] = secrets_manager;
        }
        body
    }
}

/// RULE: `CreateConnectorDestination` judges what the request CARRIES before it asks the
/// store about anything — a create with no Secrets Manager ARN is told about that field,
/// whether its `CloudConnectorId` names a connector this region holds or not, and stores
/// nothing. Supply the ARN and the next check is reached: the connector REFERENCE, which
/// states its own sentence, and a create that satisfies both is accepted.
#[tokio::test]
async fn a_create_reports_the_field_it_is_missing_before_the_reference_it_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let live = cloud_connector(&router, REGION).await;
    let absent_connector = "no-such-connector";

    for row in destination_bodies() {
        // The connector half of the request varies too: one this region holds, and one it
        // does not. The answer is the same, which is what "before the reference" means.
        for (connector_form, connector) in
            [("a live connector", live.as_str()), ("an absent connector", absent_connector)]
        {
            let case = format!("{}/{connector_form}", row.label);
            let refused = call(
                &router,
                "POST",
                "/connector-destinations",
                Some(row.body(connector, None)),
            )
            .await;
            assert_refused_with(&case, &refused, SECRETS_MANAGER_ARN_REASON);
        }
    }
    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .is_empty(),
        "a refused create must store nothing"
    );

    // With the ARN supplied, the SAME rows reach the connector reference, which states the
    // reference's own sentence for a connector this region does not hold.
    for row in destination_bodies() {
        let case = format!("{}/reference reached", row.label);
        let refused = call(
            &router,
            "POST",
            "/connector-destinations",
            Some(row.body(absent_connector, Some(secret("partner-oauth")))),
        )
        .await;
        assert_refused_with(&case, &refused, CLOUD_CONNECTOR_REFERENCE_REASON);
    }
    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .connector_destinations
            .is_empty(),
        "a create refused on its reference must store nothing either"
    );

    // And with both satisfied, the create is accepted and the destination reads back.
    for row in destination_bodies() {
        let created = call(
            &router,
            "POST",
            "/connector-destinations",
            Some(row.body(&live, Some(secret("partner-oauth")))),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: a create carrying both must be accepted, got {} with body {:?}",
            row.label, created.status, created.body
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected an Id member, got {:?}",
                    row.label, created.body
                )
            })
            .to_string();
        let read = call(
            &router,
            "GET",
            &format!("/connector-destinations/{id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the created destination must read back, got {} with body {:?}",
            row.label, read.status, read.body
        );
        assert_eq!(
            read.member("SecretsManager"),
            Some(&secret("partner-oauth")),
            "case {:?}: the read reports the stored SecretsManager, got {:?}",
            row.label,
            read.member("SecretsManager")
        );
    }
}
