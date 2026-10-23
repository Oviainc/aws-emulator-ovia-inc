// SPDX-License-Identifier: CC0-1.0

//! `GetManagedThingCapabilities` over the wire, checked over a TABLE of inputs rather
//! than over one recorded request.
//!
//! RULE — a capability read projects the STORED managed-thing record: it reports that
//! record's id plus the `Capabilities` string and the `CapabilityReport` the last
//! create or update wrote for it, member for member, reports a capability member no
//! call ever wrote as absent, and changes nothing a later caller sees. A device's
//! capabilities are a resource reached through the managed thing rather than a member
//! of it, so a request naming capabilities this service does not hold — an id no
//! managed thing in the request's region carries, or a managed thing that was never
//! given either capability member — is refused with a 404 `ResourceNotFoundException`,
//! whose reason says WHICH of the two is missing: the managed thing, or the
//! capabilities of one that does exist.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`),
//! so the routes, status codes, bodies and modelled errors asserted here are the ones
//! a client sees.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    resource_not_found_message, sub_resource_not_found_message,
    MANAGED_THING_CAPABILITIES_SUB_RESOURCE, MANAGED_THING_RESOURCE_NAME,
    MANAGED_THING_RESOURCE_TYPE,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// Another region's endpoint, used to check that a managed thing created in one region
/// is not resolved through another.
const OTHER_REGION: &str = "us-west-2";

/// The `@length` bounds of `Capabilities`
/// (`smithy_query(query="shape",shape="Capabilities")`: min 1, max 65535).
const CAPABILITIES_MIN_LENGTH: usize = 1;
const CAPABILITIES_MAX_LENGTH: usize = 65535;

/// The `@length` max of `CapabilityReportEndpoints`
/// (`smithy_query(query="shape",shape="CapabilityReportEndpoints")`: min 0, max 40).
const MAX_ENDPOINTS: usize = 40;

/// The reason a read states when the managed thing itself is not held.
fn thing_not_found() -> String {
    resource_not_found_message(MANAGED_THING_RESOURCE_NAME)
}

/// The reason a capability read states when the managed thing is held but carries no
/// capability member.
fn capabilities_not_found() -> String {
    sub_resource_not_found_message(
        MANAGED_THING_RESOURCE_NAME,
        MANAGED_THING_CAPABILITIES_SUB_RESOURCE,
    )
}

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

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine:
/// the same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request and reduce the answer. The `Authorization` credential scope is
/// what routes the request to this service and names the region it addresses.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_signed(router, REGION, method, path_and_query, body).await
}

/// [`call`], signed for an explicit region.
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

/// `CreateManagedThing` with the capability members of one case, asserting the modelled
/// 201 and returning the generated id.
async fn create(router: &MultiServiceRouter, case: &Case) -> String {
    let mut request = json!({
        "Role": "DEVICE",
        "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
        "AuthenticationMaterial": "zwave:qr:0123456789",
    });
    let members = request
        .as_object_mut()
        .expect("the create request is a JSON object");
    if let Some(capabilities) = &case.capabilities {
        members.insert("Capabilities".to_string(), json!(capabilities));
    }
    if let Some(report) = &case.report {
        members.insert("CapabilityReport".to_string(), report.clone());
    }

    let reply = call(router, "POST", "/managed-things", Some(request)).await;
    assert_eq!(
        reply.status,
        201,
        "case {:?}: expected HTTP 201 from CreateManagedThing, got {} with body {:?}",
        case.name,
        reply.status,
        reply.body
    );
    reply
        .string("Id")
        .unwrap_or_else(|| panic!("case {:?}: the create must answer with an Id", case.name))
        .to_string()
}

/// `GetManagedThingCapabilities` (`GET /managed-things-capabilities/{Identifier}`).
async fn get_capabilities(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/managed-things-capabilities/{id}"),
        None,
    )
    .await
}

/// A JSON value with every `null` object entry dropped, recursively: an absent member
/// and a member serialized as `null` say the same thing on the wire, so the comparison
/// of a reported report against the one that was stored does not depend on which.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

/// One `CapabilityReport` of `endpoints` endpoints, each carrying `capabilities`
/// capabilities. The member names, patterns and bounds are the modelled ones
/// (`smithy_query(query="shape",shape="CapabilityReport,CapabilityReportEndpoint,CapabilityReportCapability")`):
/// `version` is pattern `^1\.0\.0$`, an endpoint `id` is `^[0-9a-zA-Z]+$`, a capability
/// `id` is `^[a-zA-Z0-9.]+@(\d+\.\d+(\.\d+)?|\$latest)$` and its `version` is
/// `^(0|[1-9][0-9]*)$`.
fn report(node_id: Option<&str>, endpoints: usize, capabilities: usize) -> Value {
    let endpoints = (0..endpoints)
        .map(|endpoint| {
            json!({
                "id": format!("e{endpoint}"),
                "deviceTypes": (0..endpoint.min(2)).map(|kind| format!("Dimmer{kind}")).collect::<Vec<_>>(),
                "capabilities": (0..capabilities)
                    .map(|capability| json!({
                        "id": format!("aws.OnOff{capability}@1.0"),
                        "name": format!("On/Off {capability}"),
                        "version": format!("{capability}"),
                        "properties": ["OnOff"],
                        "actions": ["TurnOn", "TurnOff"],
                        "events": [],
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    let mut value = json!({"version": "1.0.0", "endpoints": endpoints});
    if let Some(node_id) = node_id {
        value
            .as_object_mut()
            .expect("the report is a JSON object")
            .insert("nodeId".to_string(), json!(node_id));
    }
    value
}

/// One row of the table: the capability members a call writes, and therefore the ones a
/// capability read must report.
struct Case {
    name: String,
    capabilities: Option<String>,
    report: Option<Value>,
}

impl Case {
    fn new(name: &str, capabilities: Option<&str>, report: Option<Value>) -> Self {
        Self {
            name: name.to_string(),
            capabilities: capabilities.map(str::to_string),
            report,
        }
    }
}

/// The table: capability members differing in presence, in the deprecated string's
/// length (both published `@length` bounds of `Capabilities`), and in the report's node
/// id, endpoint count (none, one, several, the published maximum) and capability count.
fn cases() -> Vec<Case> {
    vec![
        // Neither member written: there are no capabilities to report, so the read is
        // refused as not found.
        Case::new("neither-member", None, None),
        Case::new("capabilities-only", Some("light:1"), None),
        Case::new(
            "capabilities-at-min-length",
            Some(&"a".repeat(CAPABILITIES_MIN_LENGTH)),
            None,
        ),
        Case::new(
            "capabilities-at-max-length",
            Some(&"b".repeat(CAPABILITIES_MAX_LENGTH)),
            None,
        ),
        // `endpoints` is required but its list may be empty (@length min 0).
        Case::new("report-no-endpoints", None, Some(report(None, 0, 0))),
        Case::new(
            "report-endpoint-no-capabilities",
            None,
            Some(report(Some("node-1"), 1, 0)),
        ),
        Case::new("report-one-endpoint", None, Some(report(Some("node-1"), 1, 1))),
        Case::new("report-three-endpoints", None, Some(report(None, 3, 2))),
        Case::new(
            "report-at-max-endpoints",
            None,
            Some(report(Some("node-max"), MAX_ENDPOINTS, 1)),
        ),
        Case::new(
            "both-members",
            Some("light:1,dimmer:2"),
            Some(report(Some("node-2"), 2, 3)),
        ),
    ]
}

/// Assert that a capability read of `id` reports exactly `capabilities` and `report` —
/// or, when the record holds neither, that it is refused as not found.
fn assert_reports(
    case: &str,
    id: &str,
    reply: &Reply,
    capabilities: Option<&str>,
    report: Option<&Value>,
) {
    if capabilities.is_none() && report.is_none() {
        assert_not_found(case, id, reply, &capabilities_not_found());
        return;
    }
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from GetManagedThingCapabilities, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("ManagedThingId"),
        Some(id),
        "case {case:?}: expected ManagedThingId {id:?}, got {:?}",
        reply.string("ManagedThingId")
    );
    assert_eq!(
        reply.string("Capabilities"),
        capabilities,
        "case {case:?}: expected Capabilities {capabilities:?}, got {:?}",
        reply.string("Capabilities")
    );
    let reported = reply.member("CapabilityReport").map(without_nulls);
    let expected = report.map(without_nulls);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected CapabilityReport {expected:?}, got {reported:?}"
    );
}

/// RULE: a capability read projects the stored record — the id, the deprecated
/// `Capabilities` string and the `CapabilityReport` a create wrote, and nothing it did
/// not write.
#[tokio::test]
async fn a_capability_read_reports_what_the_create_stored() {
    let router = emulator();

    for case in cases() {
        let id = create(&router, &case).await;
        let reply = get_capabilities(&router, &id).await;
        assert_reports(
            &case.name,
            &id,
            &reply,
            case.capabilities.as_deref(),
            case.report.as_ref(),
        );
    }
}

/// RULE: the read projects the record as it stands, so an update's capability members
/// are what the next read reports, and an update that names neither leaves both alone.
#[tokio::test]
async fn a_capability_read_reports_what_the_last_update_stored() {
    let router = emulator();

    // Each row updates the thing created by the row before it, so every pair of
    // (stored, written) shapes in the table is exercised in turn.
    let rows = cases();
    for (index, case) in rows.iter().enumerate() {
        let target = &rows[(index + 1) % rows.len()];
        let id = create(&router, case).await;
        let update_case = format!("{} -> {}", case.name, target.name);

        let mut request = json!({});
        let members = request
            .as_object_mut()
            .expect("the update request is a JSON object");
        if let Some(capabilities) = &target.capabilities {
            members.insert("Capabilities".to_string(), json!(capabilities));
        }
        if let Some(report) = &target.report {
            members.insert("CapabilityReport".to_string(), report.clone());
        }
        let update = call(&router, "PUT", &format!("/managed-things/{id}"), Some(request)).await;
        assert_eq!(
            update.status, 204,
            "case {update_case:?}: expected HTTP 204 from UpdateManagedThing, got {} with body {:?}",
            update.status, update.body
        );

        // An update replaces the members it names and leaves absent ones alone, so what
        // the read must report is the target's member where it has one, else the
        // created record's.
        let expected_capabilities = target
            .capabilities
            .as_deref()
            .or(case.capabilities.as_deref());
        let expected_report = target.report.as_ref().or(case.report.as_ref());
        let reply = get_capabilities(&router, &id).await;
        assert_reports(
            &update_case,
            &id,
            &reply,
            expected_capabilities,
            expected_report,
        );
    }
}

/// RULE: the read is reporting-only — reading changes nothing a later caller sees, so
/// repeated reads of the same record answer identically.
#[tokio::test]
async fn repeated_capability_reads_answer_identically() {
    let router = emulator();

    for case in cases() {
        let id = create(&router, &case).await;
        let first = get_capabilities(&router, &id).await;
        let second = get_capabilities(&router, &id).await;
        let third = get_capabilities(&router, &id).await;
        assert_eq!(
            (second.status, &second.body),
            (first.status, &first.body),
            "case {:?}: a second read must answer exactly as the first did",
            case.name
        );
        assert_eq!(
            (third.status, &third.body),
            (first.status, &first.body),
            "case {:?}: a third read must answer exactly as the first did",
            case.name
        );
        // The record itself is untouched by the reads: `GetManagedThing` still reports it.
        let thing = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
        assert_eq!(
            thing.status, 200,
            "case {:?}: the managed thing must still be readable after capability reads, got {} with body {:?}",
            case.name, thing.status, thing.body
        );
    }
}

/// The 404 a capability read answers for capabilities this service does not hold: the
/// modelled `ResourceNotFoundException` (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")` -> members `Message`, `ResourceId`,
/// `ResourceType`, `httpError` 404), stated by the same read-path helpers every other
/// read of this service uses.
fn assert_not_found(case: &str, identifier: &str, reply: &Reply, message: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected the modelled error code, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("Message"),
        Some(message),
        "case {case:?}: expected the reason {message:?}, got {:?}",
        reply.string("Message")
    );
    assert_eq!(
        reply.string("ResourceId"),
        Some(identifier),
        "case {case:?}: expected ResourceId {identifier:?}, got {:?}",
        reply.string("ResourceId")
    );
    assert_eq!(
        reply.string("ResourceType"),
        Some(MANAGED_THING_RESOURCE_TYPE),
        "case {case:?}: expected ResourceType {MANAGED_THING_RESOURCE_TYPE:?}, got {:?}",
        reply.string("ResourceType")
    );
}

/// RULE: capabilities this service does not hold are refused with a 404
/// `ResourceNotFoundException`, whether the managed thing was never created, was
/// deleted, lives in another region, or holds no capability member at all.
#[tokio::test]
async fn capabilities_this_service_does_not_hold_are_refused_as_not_found() {
    let router = emulator();

    let created = create(&router, &Case::new("deleted", Some("light:1"), None)).await;
    let delete = call(&router, "DELETE", &format!("/managed-things/{created}"), None).await;
    assert_eq!(
        delete.status, 204,
        "case \"deleted\": expected HTTP 204 from DeleteManagedThing, got {} with body {:?}",
        delete.status, delete.body
    );

    let other_region =
        create(&router, &Case::new("other-region", None, Some(report(None, 1, 1)))).await;
    let no_capabilities = create(&router, &Case::new("no-capability-member", None, None)).await;

    // Each row is a managed thing whose capabilities the read cannot resolve, the region
    // its read is signed for, and the reason that absence is stated with: the managed
    // thing itself missing, or the capabilities of one that does exist.
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "never-created",
            "no-such-thing".to_string(),
            REGION,
            thing_not_found(),
        ),
        ("deleted", created, REGION, thing_not_found()),
        (
            "other-region",
            other_region,
            OTHER_REGION,
            thing_not_found(),
        ),
        (
            "no-capability-member",
            no_capabilities,
            REGION,
            capabilities_not_found(),
        ),
    ];

    for (case, identifier, region, message) in rows {
        let reply = call_signed(
            &router,
            region,
            "GET",
            &format!("/managed-things-capabilities/{identifier}"),
            None,
        )
        .await;
        assert_not_found(case, &identifier, &reply, &message);
    }
}
