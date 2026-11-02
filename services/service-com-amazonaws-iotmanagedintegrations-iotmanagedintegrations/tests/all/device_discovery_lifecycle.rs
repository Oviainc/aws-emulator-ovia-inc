// SPDX-License-Identifier: CC0-1.0

//! The device-discovery surface, end to end over the wire: `StartDeviceDiscovery` ->
//! `GetDeviceDiscovery` -> `ListDiscoveredDevices`.
//!
//! RULE: a device discovery job is a RESOURCE of this service. `StartDeviceDiscovery` (POST
//! `/device-discoveries`, success 201) mints a `DeviceDiscoveryId` for it and records which
//! surface it searched; `GetDeviceDiscovery` (GET `/device-discoveries/{Identifier}`,
//! `@readonly`) reports that record, with the `Status` the CLOCK gives it — `RUNNING` while
//! the run is not over, `SUCCEEDED` with a `FinishedAt` from its due instant — and
//! `ListDiscoveredDevices` (GET `/device-discoveries/{Identifier}/devices`, `@readonly`)
//! pages over the devices the job found. An `Identifier` the caller's region holds no job
//! under is the modelled `ResourceNotFoundException` (HTTP 404) on both reads, which is what
//! distinguishes "no such discovery job" from a job that found no devices (HTTP 200 with an
//! empty `Items`). The devices a job finds are the managed things the store holds on the
//! surface the request names — those whose `ParentControllerId` is the `ControllerIdentifier`
//! of a hub-connected discovery, and those whose `ConnectorDestinationId` is the connector
//! destination the `AccountAssociationId` of a third-party-connected discovery points at —
//! narrowed by `ConnectorDeviceIdList` where the request carries one, and reported from the
//! STORED record of each device. A device no earlier job of the region reported is
//! `DISCOVERED`; one an earlier job reported is `NO_CHANGE`, or `UPDATED` when the record
//! changed after that job started.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.device-discovery-lifecycle-and-discovered-devices`) is ONE row
//! of the tables below. The rows differ along what the rule generalises over: the
//! `DiscoveryType` (the model's whole value set), which surface the request names, how many
//! devices are attached to it, whether the device carries a connector device id, the filter
//! list, the page size (including both modelled `MaxResults` bounds), the form of an
//! `Identifier` nothing allocated, and the region the request is signed for.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::DISCOVERY_TYPE_VALUES;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    DEVICE_DISCOVERY_DURATION, DEVICE_DISCOVERY_RESOURCE_TYPE, DEVICE_DISCOVERY_STATUS_RUNNING,
    DEVICE_DISCOVERY_STATUS_SUCCEEDED, DISCOVERED_DEVICE_PAGE_LIMIT,
    DISCOVERY_MODIFICATION_DISCOVERED, DISCOVERY_MODIFICATION_NO_CHANGE,
    DISCOVERY_MODIFICATION_UPDATED, MAX_DEVICE_DISCOVERIES,
    MAX_DISCOVERED_DEVICES_PER_DISCOVERY,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The modelled `MaxResults` range of `ListDiscoveredDevices`
/// (`smithy_query(query="operation",operation="ListDiscoveredDevices")` -> range 1..1000).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 1000);

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

    fn seconds(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }

    /// The `Items` of a list answer.
    fn items(&self) -> Vec<Value> {
        self.member("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `ManagedThingId` of every item, in the order the page reports them.
    fn discovered_ids(&self) -> Vec<String> {
        self.items()
            .iter()
            .map(|item| {
                item.get("ManagedThingId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the caller keeps a handle to, so a test can seed the
/// service's store directly instead of driving thousands of requests.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
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

/// `StartDeviceDiscovery` with `body`, signed for `region`, asserting the modelled 201 and
/// answering the `DeviceDiscoveryId` it minted — checked against its own modelled shape.
async fn start_in(router: &MultiServiceRouter, region: &str, case: &str, body: Value) -> String {
    let started = call_in(router, region, "POST", "/device-discoveries", Some(body)).await;
    assert_eq!(
        started.status, 201,
        "case {case:?}: expected HTTP 201 from StartDeviceDiscovery, got {} with body {:?}",
        started.status, started.body
    );
    let id = started.string("Id").unwrap_or_default().to_string();
    assert!(
        // `DeviceDiscoveryId` is length 1..200, pattern ^[A-Za-z0-9]+$.
        (1..=200).contains(&id.chars().count()) && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected a DeviceDiscoveryId of 1..=200 alphanumerics, got {id:?} in \
         body {:?}",
        started.body
    );
    assert!(
        started.seconds("StartedAt").is_some_and(|secs| secs > 0.0),
        "case {case:?}: expected a StartedAt timestamp, got {:?}",
        started.member("StartedAt")
    );
    id
}

/// [`start_in`] in [`REGION`].
async fn start(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    start_in(router, REGION, case, body).await
}

/// `GetDeviceDiscovery` for `id`, signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, id: &str) -> Reply {
    call_in(
        router,
        region,
        "GET",
        &format!("/device-discoveries/{id}"),
        None,
    )
    .await
}

/// [`get_in`] in [`REGION`].
async fn get(router: &MultiServiceRouter, id: &str) -> Reply {
    get_in(router, REGION, id).await
}

/// `ListDiscoveredDevices` for `id` with `query` appended, signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, id: &str, query: &str) -> Reply {
    call_in(
        router,
        region,
        "GET",
        &format!("/device-discoveries/{id}/devices{query}"),
        None,
    )
    .await
}

/// [`list_in`] in [`REGION`], with no query string.
async fn list(router: &MultiServiceRouter, id: &str) -> Reply {
    list_in(router, REGION, id, "").await
}

/// Create a managed thing carrying `meta_data` and the optional members `extra`, and answer
/// its generated id.
async fn thing_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    meta_data: Value,
    extra: Value,
) -> String {
    let mut body = json!({
        "Role": "DEVICE",
        "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
        "AuthenticationMaterial": "zwave:qr:0123456789",
        "MetaData": meta_data,
    });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    let created = call_in(router, region, "POST", "/managed-things", Some(body)).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateManagedThing, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected CreateManagedThing to answer an Id, got body {:?}",
                created.body
            )
        })
        .to_string()
}

/// [`thing_in`] in [`REGION`], carrying only metadata.
async fn thing(router: &MultiServiceRouter, case: &str, meta_data: Value) -> String {
    thing_in(router, REGION, case, meta_data, json!({})).await
}

/// Create a connector destination in [`REGION`] through the wire and answer the
/// `ConnectorDestinationId` it minted.
///
/// `CreateAccountAssociation` resolves its `ConnectorDestinationId` against this service's
/// own connector-destination store, so every association below is created against a
/// destination a client really created; a connector destination in turn names a C2C
/// connector (`CloudConnectorId` is `@required`), so the connector comes first.
async fn connector_destination(router: &MultiServiceRouter, case: &str) -> String {
    let connector = call_in(
        router,
        REGION,
        "POST",
        "/cloud-connectors",
        Some(json!({
            "Name": "partner cloud connector",
            "EndpointConfig": {"lambda": {"arn": "connector-handler"}},
        })),
    )
    .await;
    assert_eq!(
        connector.status, 201,
        "case {case:?}: seeding a cloud connector expected HTTP 201, got {} with body {:?}",
        connector.status, connector.body
    );
    let connector_id = connector.string("Id").unwrap_or_default().to_string();
    let destination = call_in(
        router,
        REGION,
        "POST",
        "/connector-destinations",
        Some(json!({
            "CloudConnectorId": connector_id,
            "AuthConfig": {},
            // The service requires the ARN of the secret a destination keeps its
            // credentials in. Inside the `SecretsManagerArn` `@pattern`
            // (`^arn:aws:secretsmanager:[0-9a-zA-Z-]{1,32}:\d{12}:secret:[A-Za-z0-9/_+=.@-]{8,520}$`)
            // and the `SecretsManagerVersionId` `@length` (32..64).
            "SecretsManager": {
                "arn": format!(
                    "arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:destination-A1b2C3"
                ),
                "versionId": "a".repeat(32),
            },
        })),
    )
    .await;
    assert_eq!(
        destination.status, 201,
        "case {case:?}: seeding a connector destination expected HTTP 201, got {} with \
         body {:?}",
        destination.status, destination.body
    );
    destination.string("Id").unwrap_or_default().to_string()
}

/// Create an account association against `destination_id` and answer its generated id.
async fn account_association(
    router: &MultiServiceRouter,
    case: &str,
    destination_id: &str,
) -> String {
    let created = call_in(
        router,
        REGION,
        "POST",
        "/account-associations",
        Some(json!({"ConnectorDestinationId": destination_id})),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateAccountAssociation, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("AccountAssociationId")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an AccountAssociationId, got body {:?}",
                created.body
            )
        })
        .to_string()
}

/// Assert one answer is the modelled refusal `status`/`code` carrying a reason.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?}",
        reply.error_type
    );
    assert!(
        !reply.message().is_empty(),
        "case {case:?}: expected the modelled Message member to carry a reason, got body {:?}",
        reply.body
    );
}

/// Assert one list reports exactly `expected` managed thing ids, in order.
fn assert_discovered(case: &str, reply: &Reply, expected: &[String]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        reply.status, reply.body
    );
    let actual = reply.discovered_ids();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the discovered devices {expected:?}, got {actual:?}"
    );
}

/// The `Modification` of every item, in the order the page reports them.
fn modifications(reply: &Reply) -> Vec<String> {
    reply
        .items()
        .iter()
        .map(|item| {
            item.get("Modification")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The surface member a discovery of `discovery_type` names, so one table can span every
/// `DiscoveryType` the model declares: the hub protocols and the controller rediscovery name
/// a controller, and the cloud types name an account association.
fn surface(discovery_type: &str, controller: &str, association: &str) -> Value {
    match discovery_type {
        "CLOUD" | "CUSTOM" => json!({
            "DiscoveryType": discovery_type,
            "AccountAssociationId": association,
            // `CustomProtocolDetail` is the protocol-specific detail a CUSTOM discovery
            // carries; the model documents a `Name` key for it.
            "CustomProtocolDetail": {"Name": "custom"},
        }),
        _ => json!({
            "DiscoveryType": discovery_type,
            "ControllerIdentifier": controller,
        }),
    }
}

/// RULE: a start mints a job, a get reports it with the status the clock gives it, and the
/// job moves from `RUNNING` to `SUCCEEDED` at its due instant — for every `DiscoveryType`
/// the model declares.
#[tokio::test]
async fn a_started_job_is_running_until_the_clock_finishes_it() {
    let router = emulator();
    let controller = thing(&router, "controller", json!({"ParentControllerId": "none"})).await;
    let destination = connector_destination(&router, "association").await;
    let association = account_association(&router, "association", &destination).await;

    // Every row starts a job of one discovery type; the jobs are then read together, so the
    // whole table observes one run of the clock.
    let mut started: Vec<(&str, String)> = Vec::new();
    for discovery_type in DISCOVERY_TYPE_VALUES {
        let id = start(
            &router,
            discovery_type,
            surface(discovery_type, &controller, &association),
        )
        .await;
        started.push((discovery_type, id));
    }

    for (discovery_type, id) in &started {
        let read = get(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {discovery_type:?}: expected HTTP 200 from GetDeviceDiscovery, got {} with \
             body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("Id"),
            Some(id.as_str()),
            "case {discovery_type:?}: expected the job's own Id, got {:?}",
            read.string("Id")
        );
        assert_eq!(
            read.string("Arn"),
            Some(
                format!("arn:aws:iotmanagedintegrations:{REGION}:{ACCOUNT}:device-discovery/{id}")
                    .as_str()
            ),
            "case {discovery_type:?}: expected the device-discovery ARN of the minted id, \
             got {:?}",
            read.string("Arn")
        );
        assert_eq!(
            read.string("DiscoveryType"),
            Some(*discovery_type),
            "case {discovery_type:?}: expected the discovery type the start named, got {:?}",
            read.string("DiscoveryType")
        );
        // The run outlives the call that started it, so a caller reading straight after the
        // start still sees the job running and no finish time.
        assert_eq!(
            read.string("Status"),
            Some(DEVICE_DISCOVERY_STATUS_RUNNING),
            "case {discovery_type:?}: expected the job to still be running, got {:?}",
            read.string("Status")
        );
        assert!(
            read.member("FinishedAt").is_none(),
            "case {discovery_type:?}: a running job has not finished, got {:?}",
            read.member("FinishedAt")
        );
    }

    // Past the due instant every job reports as succeeded, with a finish time — and nothing
    // had to call anything for the transition to become due.
    tokio::time::sleep(DEVICE_DISCOVERY_DURATION + Duration::from_millis(100)).await;
    for (discovery_type, id) in &started {
        let read = get(&router, id).await;
        assert_eq!(
            read.string("Status"),
            Some(DEVICE_DISCOVERY_STATUS_SUCCEEDED),
            "case {discovery_type:?}: expected the job to have succeeded, got {:?}",
            read.string("Status")
        );
        let finished = read.seconds("FinishedAt");
        let started_at = read.seconds("StartedAt");
        assert!(
            finished.is_some_and(|at| started_at.is_some_and(|start| at >= start)),
            "case {discovery_type:?}: expected a FinishedAt at or after StartedAt, got {:?} \
             and {:?}",
            read.member("FinishedAt"),
            read.member("StartedAt")
        );
    }
}

/// RULE: an `Identifier` the caller's region holds no job under is the modelled 404 on both
/// reads of the surface, which is what tells a client "no such discovery job" apart from a
/// job that found no devices.
#[tokio::test]
async fn an_identifier_no_job_was_allocated_under_is_the_modelled_not_found() {
    let router = emulator();

    // A job started through another region's endpoint, to address from this one, and one of
    // this region's own that found nothing.
    let elsewhere = start_in(
        &router,
        "eu-central-1",
        "eu-central-1 job",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": "nosuchcontroller"}),
    )
    .await;
    let empty = start(
        &router,
        "a job with nothing on its surface",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": "nosuchcontroller"}),
    )
    .await;

    // The rows differ along why this region holds no job under the identifier: nothing ever
    // allocated it (in several forms the `DeviceDiscoveryId` pattern `^[A-Za-z0-9]+$`
    // admits), or it belongs to another region's endpoint.
    let cases: Vec<(&str, String)> = vec![
        ("never allocated", "sim2realbogusdiscoveryid0000".to_string()),
        ("never allocated, one character", "a".to_string()),
        ("never allocated, digits", "0123456789".to_string()),
        ("never allocated, at the modelled maximum", "b".repeat(200)),
        ("another region's job", elsewhere.clone()),
    ];

    for (why, identifier) in cases {
        for (operation, reply) in [
            ("ListDiscoveredDevices", list(&router, &identifier).await),
            ("GetDeviceDiscovery", get(&router, &identifier).await),
        ] {
            let case = format!("{why}/{operation}");
            assert_refused(&case, &reply, 404, NOT_FOUND);
            assert_eq!(
                reply.string("ResourceType"),
                Some(DEVICE_DISCOVERY_RESOURCE_TYPE),
                "case {case:?}: expected the 404 to name the device-discovery resource type, \
                 got {:?}",
                reply.string("ResourceType")
            );
            assert_eq!(
                reply.string("ResourceId"),
                Some(identifier.as_str()),
                "case {case:?}: expected the 404 to report the identifier the request carried, \
                 got {:?}",
                reply.string("ResourceId")
            );
        }
    }

    // A job that EXISTS and found no devices answers 200 with an empty list and no token —
    // the answer a client must be able to tell from the 404 above.
    let listed = list(&router, &empty).await;
    assert_discovered("a job with nothing on its surface", &listed, &[]);
    assert!(
        listed.member("NextToken").is_none(),
        "case \"a job with nothing on its surface\": a complete page carries no NextToken, \
         got {:?}",
        listed.member("NextToken")
    );

    // And the other region's job still resolves through its own endpoint.
    let there = list_in(&router, "eu-central-1", &elsewhere, "").await;
    assert_discovered("eu-central-1 read back", &there, &[]);
}

/// RULE: a hub-connected discovery reports the managed things attached to the controller it
/// names — and only those — projecting each one from its stored record, with the
/// modification an earlier discovery of the same device gives it.
#[tokio::test]
async fn a_hub_discovery_reports_the_devices_attached_to_the_controller_it_names() {
    let router = emulator();

    let hub = thing(&router, "hub", json!({"ParentControllerId": "none"})).await;
    let other_hub = thing(&router, "other hub", json!({"ParentControllerId": "none"})).await;

    // Three devices on the hub's network, differing along which attributes their records
    // carry: a bare one, one with a connector device id, and one with brand, model and
    // classification (which is what a discovered device reports as its device type).
    let bare = thing(&router, "bare device", json!({"ParentControllerId": hub.clone()})).await;
    let identified = thing(
        &router,
        "identified device",
        json!({"ParentControllerId": hub.clone(), "ConnectorDeviceId": "device.1"}),
    )
    .await;
    let described = thing_in(
        &router,
        REGION,
        "described device",
        json!({"ParentControllerId": hub.clone(), "ConnectorDeviceId": "device.2"}),
        json!({"Brand": "Acme", "Model": "L1", "Name": "Hall light", "Classification": "light"}),
    )
    .await;
    // Devices that are NOT on this hub's network: one attached to nothing, one attached to
    // another controller.
    let detached = thing(&router, "detached device", json!({"room": "hall"})).await;
    let elsewhere = thing(
        &router,
        "another hub's device",
        json!({"ParentControllerId": other_hub.clone()}),
    )
    .await;

    let attached = vec![bare.clone(), identified.clone(), described.clone()];

    // Every hub-connected discovery type searches the controller it names.
    for discovery_type in ["ZWAVE", "ZIGBEE", "CONTROLLER_CAPABILITY_REDISCOVERY"] {
        let case = format!("{discovery_type} on the hub");
        let job = start(
            &router,
            &case,
            json!({"DiscoveryType": discovery_type, "ControllerIdentifier": hub.clone()}),
        )
        .await;
        let listed = list(&router, &job).await;
        assert_discovered(&case, &listed, &attached);
        for absent in [&detached, &elsewhere, &hub] {
            assert!(
                !listed.discovered_ids().contains(absent),
                "case {case:?}: {absent:?} is not on this controller's network, but was \
                 reported: {:?}",
                listed.discovered_ids()
            );
        }
    }

    // The members of a reported device come from its stored record.
    let job = start(
        &router,
        "projection",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": hub.clone()}),
    )
    .await;
    let listed = list(&router, &job).await;
    let described_item = listed
        .items()
        .into_iter()
        .find(|item| item.get("ManagedThingId").and_then(Value::as_str) == Some(described.as_str()))
        .expect("the described device among the reported ones");
    for (member, expected) in [
        ("ConnectorDeviceId", json!("device.2")),
        ("ConnectorDeviceName", json!("Hall light")),
        ("Brand", json!("Acme")),
        ("Model", json!("L1")),
        ("DeviceTypes", json!(["light"])),
    ] {
        assert_eq!(
            described_item.get(member),
            Some(&expected),
            "case \"projection\": expected {member} to be the stored {expected:?}, got {:?}",
            described_item.get(member)
        );
    }
    assert!(
        described_item
            .get("DiscoveredAt")
            .and_then(Value::as_f64)
            .is_some_and(|secs| secs > 0.0),
        "case \"projection\": expected a DiscoveredAt timestamp, got {:?}",
        described_item.get("DiscoveredAt")
    );
    let bare_item = listed
        .items()
        .into_iter()
        .find(|item| item.get("ManagedThingId").and_then(Value::as_str) == Some(bare.as_str()))
        .expect("the bare device among the reported ones");
    for member in ["ConnectorDeviceId", "Brand", "Model", "DeviceTypes"] {
        assert!(
            bare_item.get(member).is_none_or(Value::is_null),
            "case \"projection\": a record carrying no {member} reports none, got {:?}",
            bare_item.get(member)
        );
    }

    // `ConnectorDeviceIdList` is a FILTER: it keeps only the devices carrying one of the
    // ids it names. The rows differ along how many ids the list carries.
    for (case, wanted, expected) in [
        ("one id", vec!["device.1"], vec![identified.clone()]),
        (
            "two ids",
            vec!["device.1", "device.2"],
            vec![identified.clone(), described.clone()],
        ),
        ("an id no device carries", vec!["device.9"], vec![]),
    ] {
        let job = start(
            &router,
            case,
            json!({
                "DiscoveryType": "ZWAVE",
                "ControllerIdentifier": hub.clone(),
                "ConnectorDeviceIdList": wanted,
            }),
        )
        .await;
        assert_discovered(case, &list(&router, &job).await, &expected);
    }

    // A device no earlier job reported is DISCOVERED; the jobs above already reported these
    // three, so the next job reports them unchanged.
    let repeat = start(
        &router,
        "a repeated discovery",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": hub.clone()}),
    )
    .await;
    let listed = list(&router, &repeat).await;
    assert_eq!(
        modifications(&listed),
        vec![DISCOVERY_MODIFICATION_NO_CHANGE.to_string(); attached.len()],
        "case \"a repeated discovery\": expected every device to be unchanged, got {:?}",
        modifications(&listed)
    );

    // Changing one device's record makes the next discovery report it as UPDATED, and the
    // others still unchanged.
    let updated = call_in(
        &router,
        REGION,
        "PUT",
        &format!("/managed-things/{bare}"),
        Some(json!({"Name": "Renamed"})),
    )
    .await;
    assert_eq!(
        updated.status, 204,
        "case \"an updated device\": expected HTTP 204 from UpdateManagedThing, got {} with \
         body {:?}",
        updated.status, updated.body
    );
    let after = start(
        &router,
        "a discovery after an update",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": hub.clone()}),
    )
    .await;
    let listed = list(&router, &after).await;
    assert_eq!(
        modifications(&listed),
        vec![
            DISCOVERY_MODIFICATION_UPDATED.to_string(),
            DISCOVERY_MODIFICATION_NO_CHANGE.to_string(),
            DISCOVERY_MODIFICATION_NO_CHANGE.to_string(),
        ],
        "case \"a discovery after an update\": expected only the changed device to be \
         UPDATED, got {:?}",
        modifications(&listed)
    );

    // A device attached after a job started is not in that job's result — a discovery
    // reports the surface as it was when it ran — and the next job reports it as DISCOVERED.
    let fresh = thing(&router, "a later device", json!({"ParentControllerId": hub.clone()})).await;
    assert_discovered("the earlier job is a snapshot", &list(&router, &after).await, &attached);
    let latest = start(
        &router,
        "a discovery after an attachment",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": hub.clone()}),
    )
    .await;
    let listed = list(&router, &latest).await;
    let mut expected = attached.clone();
    expected.push(fresh.clone());
    assert_discovered("a discovery after an attachment", &listed, &expected);
    assert_eq!(
        modifications(&listed).last().map(String::as_str),
        Some(DISCOVERY_MODIFICATION_DISCOVERED),
        "case \"a discovery after an attachment\": expected the new device to be DISCOVERED, \
         got {:?}",
        modifications(&listed)
    );
}

/// RULE: a third-party-connected discovery reports the managed things whose connector
/// destination is the one the account association it names was created against — and a
/// request that names no surface this store knows discovers nothing rather than being
/// refused.
#[tokio::test]
async fn a_cloud_discovery_reports_the_devices_of_the_account_association_it_names() {
    let router = emulator();

    let one = connector_destination(&router, "association").await;
    let two = connector_destination(&router, "other association").await;
    let association = account_association(&router, "association", &one).await;
    let other = account_association(&router, "other association", &two).await;

    let first = thing(
        &router,
        "first cloud device",
        json!({"ConnectorDestinationId": one, "ConnectorDeviceId": "cloud.1"}),
    )
    .await;
    let second = thing(
        &router,
        "second cloud device",
        json!({"ConnectorDestinationId": one, "ConnectorDeviceId": "cloud.2"}),
    )
    .await;
    let elsewhere = thing(
        &router,
        "another destination's device",
        json!({"ConnectorDestinationId": two}),
    )
    .await;

    for discovery_type in ["CLOUD", "CUSTOM"] {
        let case = format!("{discovery_type} on the association");
        let job = start(
            &router,
            &case,
            surface(discovery_type, "unused", &association),
        )
        .await;
        assert_discovered(
            &case,
            &list(&router, &job).await,
            &[first.clone(), second.clone()],
        );
    }

    // The other association points at another destination, so it discovers its own device.
    let job = start(&router, "the other association", surface("CLOUD", "unused", &other)).await;
    assert_discovered(
        "the other association",
        &list(&router, &job).await,
        std::slice::from_ref(&elsewhere),
    );

    // A surface this store knows nothing about discovers nothing: the job is started, not
    // refused, and its list is the empty page rather than a 404.
    for (case, body) in [
        (
            "an account association that does not exist",
            json!({"DiscoveryType": "CLOUD", "AccountAssociationId": "nosuchassociation"}),
        ),
        (
            "no surface named at all",
            json!({"DiscoveryType": "CLOUD"}),
        ),
        (
            "a controller named on a cloud discovery",
            json!({"DiscoveryType": "CLOUD", "ControllerIdentifier": "nosuchcontroller"}),
        ),
    ] {
        let job = start(&router, case, body).await;
        assert_discovered(case, &list(&router, &job).await, &[]);
    }
}

/// RULE: `ListDeviceDiscoveries` reports this region's jobs in the order they were started,
/// each with the status the clock gives it, and honours the `TypeFilter` and `StatusFilter`
/// the model declares.
#[tokio::test]
async fn a_list_of_the_jobs_reports_this_region_s_jobs_with_their_status() {
    let router = emulator();

    // One job per discovery type, started in a known order.
    let mut started: Vec<(&str, String)> = Vec::new();
    for discovery_type in DISCOVERY_TYPE_VALUES {
        let id = start(
            &router,
            discovery_type,
            surface(discovery_type, "nosuchcontroller", "nosuchassociation"),
        )
        .await;
        started.push((discovery_type, id));
    }
    // And one in another region, which this region's list must not report.
    let elsewhere = start_in(
        &router,
        "eu-central-1",
        "eu-central-1 job",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": "nosuchcontroller"}),
    )
    .await;

    let listed = call_in(&router, REGION, "GET", "/device-discoveries", None).await;
    assert_eq!(
        listed.status, 200,
        "case \"all\": expected HTTP 200 from ListDeviceDiscoveries, got {} with body {:?}",
        listed.status, listed.body
    );
    let ids: Vec<String> = listed
        .items()
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let expected: Vec<String> = started.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(
        ids, expected,
        "case \"all\": expected this region's jobs in the order they were started, got {ids:?}"
    );
    assert!(
        !ids.contains(&elsewhere),
        "case \"all\": another region's job is not reported here, got {ids:?}"
    );
    for item in listed.items() {
        assert_eq!(
            item.get("Status").and_then(Value::as_str),
            Some(DEVICE_DISCOVERY_STATUS_RUNNING),
            "case \"all\": a job whose run is not over reports as running, got {item:?}"
        );
    }

    // `TypeFilter` selects by discovery type, and `StatusFilter` by the status the clock
    // gives the job — so a filter on the status no job is in reports none.
    for (discovery_type, id) in &started {
        let filtered = call_in(
            &router,
            REGION,
            "GET",
            &format!("/device-discoveries?TypeFilter={discovery_type}"),
            None,
        )
        .await;
        let ids: Vec<String> = filtered
            .items()
            .iter()
            .map(|item| {
                item.get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids,
            vec![id.clone()],
            "case {discovery_type:?}: expected the filter to select only that type's job, \
             got {ids:?}"
        );
    }
    let succeeded = call_in(
        &router,
        REGION,
        "GET",
        &format!("/device-discoveries?StatusFilter={DEVICE_DISCOVERY_STATUS_SUCCEEDED}"),
        None,
    )
    .await;
    assert!(
        succeeded.items().is_empty(),
        "case \"StatusFilter SUCCEEDED\": no job has finished yet, got {:?}",
        succeeded.items()
    );
    let running = call_in(
        &router,
        REGION,
        "GET",
        &format!("/device-discoveries?StatusFilter={DEVICE_DISCOVERY_STATUS_RUNNING}"),
        None,
    )
    .await;
    assert_eq!(
        running.items().len(),
        started.len(),
        "case \"StatusFilter RUNNING\": expected every started job, got {:?}",
        running.items()
    );

    // Past the due instant the same jobs report as succeeded, so the filters follow the
    // clock rather than a stored status.
    tokio::time::sleep(DEVICE_DISCOVERY_DURATION + Duration::from_millis(100)).await;
    let succeeded = call_in(
        &router,
        REGION,
        "GET",
        &format!("/device-discoveries?StatusFilter={DEVICE_DISCOVERY_STATUS_SUCCEEDED}"),
        None,
    )
    .await;
    assert_eq!(
        succeeded.items().len(),
        started.len(),
        "case \"StatusFilter SUCCEEDED after the run\": expected every job, got {:?}",
        succeeded.items()
    );
}

/// RULE: a list pages over the job's devices with `MaxResults` and an opaque, resumable
/// `NextToken`, and a token this service did not issue is the modelled 400.
#[tokio::test]
async fn a_list_pages_over_the_devices_with_a_resumable_token() {
    let router = emulator();

    let hub = thing(&router, "hub", json!({"ParentControllerId": "none"})).await;
    let mut devices: Vec<String> = Vec::new();
    for index in 0..5 {
        devices.push(
            thing(
                &router,
                &format!("device {index}"),
                json!({"ParentControllerId": hub.clone()}),
            )
            .await,
        );
    }
    let job = start(
        &router,
        "paging",
        json!({"DiscoveryType": "ZIGBEE", "ControllerIdentifier": hub.clone()}),
    )
    .await;

    // The rows differ along the page size: both modelled `MaxResults` bounds, the sizes in
    // between, and the absent `MaxResults` (which gets the page limit).
    let sizes: Vec<Option<i64>> = vec![
        Some(MAX_RESULTS_BOUNDS.0),
        Some(2),
        Some(4),
        Some(5),
        Some(MAX_RESULTS_BOUNDS.1),
        None,
    ];
    for size in sizes {
        let case = format!("MaxResults {size:?}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One extra round protects against a token that never ends.
        for _ in 0..=devices.len() + 1 {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = size {
                query.push(format!("MaxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("NextToken={token}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let page = list_in(&router, REGION, &job, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {:?}",
                page.status, page.body
            );
            let expected_page = size
                .map_or(DISCOVERED_DEVICE_PAGE_LIMIT, |size| {
                    usize::try_from(size).unwrap_or(1)
                })
                .min(devices.len() - collected.len());
            assert_eq!(
                page.items().len(),
                expected_page,
                "case {case:?}: expected a page of {expected_page} devices, got {:?}",
                page.discovered_ids()
            );
            collected.extend(page.discovered_ids());
            token = page.string("NextToken").map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: expected the paging to end, but a NextToken is still offered"
        );
        assert_eq!(
            collected, devices,
            "case {case:?}: expected the pages to assemble every device exactly once, got {collected:?}"
        );
    }

    // A `NextToken` this service did not issue is a client error rather than an empty page.
    for token in ["notatoken", "ffffffffffffffff", "0"] {
        let refused = list_in(&router, REGION, &job, &format!("?NextToken={token}")).await;
        assert_refused(token, &refused, 400, VALIDATION);
    }
}

/// RULE: what one start retains is bounded — a job records at most
/// `MAX_DISCOVERED_DEVICES_PER_DISCOVERY` devices however many are on its surface, and the
/// store holds at most `MAX_DEVICE_DISCOVERIES` jobs, evicting the oldest rather than
/// growing past it — so neither the work a start does nor the memory it retains grows with
/// the product of the two stores.
#[tokio::test]
async fn one_start_retains_a_bounded_snapshot_and_the_store_holds_bounded_jobs() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let hub = thing(&router, "hub", json!({"ParentControllerId": "none"})).await;
    // One real device on the hub's network, then the store filled past the per-job cap from
    // that record, so the boundary is exercised without driving thousands of requests.
    let seed = thing(&router, "seed device", json!({"ParentControllerId": hub.clone()})).await;
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        let template = store
            .managed_things
            .get(&seed)
            .cloned()
            .expect("the seeded record is stored");
        while store.managed_things.len() < MAX_DISCOVERED_DEVICES_PER_DISCOVERY + 5 {
            let sequence = store.take_sequence();
            let id = format!("filler{sequence}");
            let mut filler = template.clone();
            filler.id = id.clone();
            filler.sequence = sequence;
            store.managed_things.insert(id.clone(), filler);
            store.managed_thing_order.push(id);
        }
    }

    let job = start(
        &router,
        "a surface past the per-job cap",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": hub.clone()}),
    )
    .await;
    // Paging over the job reports exactly the cap, not the whole surface.
    let mut collected = 0usize;
    let mut token: Option<String> = None;
    loop {
        let query = token
            .as_ref()
            .map_or_else(String::new, |token| format!("?NextToken={token}"));
        let page = list_in(&router, REGION, &job, &query).await;
        assert_eq!(
            page.status, 200,
            "case \"a surface past the per-job cap\": expected HTTP 200, got {} with body {:?}",
            page.status, page.body
        );
        collected += page.items().len();
        token = page.string("NextToken").map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        collected, MAX_DISCOVERED_DEVICES_PER_DISCOVERY,
        "case \"a surface past the per-job cap\": expected the snapshot to stop at the cap, \
         got {collected} devices"
    );

    // The job store is bounded the same way: filled to its maximum, one more start evicts
    // the oldest job rather than growing the store or refusing the call.
    let oldest = job.clone();
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        let template = store
            .device_discoveries
            .get(&oldest)
            .cloned()
            .expect("the started job is stored");
        while store.device_discoveries.len() < MAX_DEVICE_DISCOVERIES {
            let sequence = store.take_sequence();
            let id = format!("filler{sequence}");
            let mut filler = template.clone();
            filler.id = id.clone();
            filler.sequence = sequence;
            store.device_discoveries.insert(id, filler);
        }
    }

    let newest = start(
        &router,
        "at the job maximum",
        json!({"DiscoveryType": "ZWAVE", "ControllerIdentifier": hub.clone()}),
    )
    .await;
    let state = engine.state::<ServiceState>();
    let held = state.lock().device_discoveries.len();
    assert!(
        held <= MAX_DEVICE_DISCOVERIES,
        "case \"at the job maximum\": expected the store to stay within its maximum of \
         {MAX_DEVICE_DISCOVERIES}, got {held}"
    );
    assert_eq!(
        get(&router, &oldest).await.status,
        404,
        "case \"at the job maximum\": the oldest job was evicted to make room"
    );
    assert_eq!(
        get(&router, &newest).await.status,
        200,
        "case \"at the job maximum\": the job just started is held"
    );
}
