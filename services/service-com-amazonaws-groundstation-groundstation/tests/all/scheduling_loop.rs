// SPDX-License-Identifier: CC0-1.0

//! The AWS Ground Station scheduling loop, end to end over HTTP.
//!
//! Every assertion here goes through the real front door: a [`MultiServiceRouter`]
//! holding this service, fed `restJson1` requests signed into the `groundstation`
//! scope, so routing, deserialization, constraint validation, the handler and
//! serialization all run exactly as they do in the emulator binary.
//!
//! The suites assert RULES over a table of inputs rather than one recorded call:
//!
//! * a config's `configType` is whichever member of the `ConfigTypeData` union the
//!   caller set, for all eight members, and `GetConfig` returns what was created;
//! * a mission profile echoes back every member it was created with, and refuses a
//!   reference to a config that does not exist;
//! * a contact window must run forwards and reach the profile's minimum viable
//!   duration;
//! * a listing returns exactly the resources matching its filters, one page at a time;
//! * a listing is bounded to one page even when the caller names no page size, and a
//!   store that reached its bound refuses the next create.
//!
//! The harness at the top of this file (`harness`, `send`, `Reply` and the create
//! helpers) is `pub(crate)`, so a sibling suite under `tests/all/` reuses it rather than
//! standing up a second copy of the same router.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_ground_stations, service_state, MAX_PAGE_SIZE, MAX_RECORDS_PER_STORE,
    SCHEDULING_SETTLES_AFTER,
};
use service_com_amazonaws_groundstation_groundstation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header in the `groundstation` scope: the routing signal the
/// emulator reads first, and the source of the region every minted ARN carries.
pub(crate) const AUTH: &str = "AWS4-HMAC-SHA256 Credential=AKIDTESTGROUNDSTATION/20260101/us-east-1/\
                    groundstation/aws4_request, SignedHeaders=host, Signature=abcdef";

/// The region the header above signs for; the ARNs this service mints carry it.
pub(crate) const REGION: &str = "us-east-1";

/// A well-formed `ConfigArn` (model pattern) for a config that was never created.
pub(crate) const ABSENT_CONFIG_ARN: &str =
    "arn:aws:groundstation:us-east-1:000000000000:config/tracking/\
     00000000-0000-0000-0000-000000000000";

/// A well-formed `MissionProfileArn` for a mission profile that was never created.
pub(crate) const ABSENT_MISSION_PROFILE_ARN: &str =
    "arn:aws:groundstation:us-east-1:000000000000:mission-profile/\
     00000000-0000-0000-0000-000000000000";

/// A well-formed `Uuid` that names nothing.
pub(crate) const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// Longer than every settle span this service chooses — the `SCHEDULING` ->
/// `SCHEDULED` move the scheduler owes a reservation, the `UPDATING` -> `ACTIVE` move it
/// owes a contact version and the `CANCELLING` -> `CANCELLED` move it owes a stopped
/// pass, which are one and the same span — so a read taken after it observes the move
/// that was owed. Derived from that span rather than written out, so it cannot drift
/// from what the service actually owes.
///
/// Waited out with `std::thread::sleep` rather than `tokio::time::sleep`: the emulator
/// stores a due instant and never arms a timer, so there is no task to drive while
/// waiting, and this workspace's `tokio` carries no `time` feature.
pub(crate) const AFTER_SETTLE: Duration =
    SCHEDULING_SETTLES_AFTER.saturating_add(Duration::from_millis(600));

/// The uplink band this suite grants the account before creating an antenna-uplink
/// config: a sub-band of the 2025–2120 MHz the service operates
/// (`smithy_query(query="doc", shape="UplinkSpectrumConfig", member="centerFrequency")`)
/// that contains [`antenna_uplink_config_data`]'s center frequency.
const UPLINK_APPROVAL_MHZ: (f64, f64) = (2025.0, 2120.0);

/// The bucket the table's `s3RecordingConfig` records to.
///
/// Ground Station resolves a recording bucket against S3 before it will create the
/// config, and this router mounts no S3: nothing is then known about the bucket, so the
/// create proceeds. The suite that asserts what a bucket S3 DOES answer for means is
/// `tests/all/s3_recording_buckets.rs`, which mounts S3 beside this service.
pub(crate) const RECORDING_BUCKET: &str = "ground-station-downlink";

/// One HTTP answer, reduced to what these tests assert on.
///
/// `pub(crate)` here and below: the sibling suites in this target reach the same front
/// door, so the plumbing is written once and shared rather than copied per file.
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The `x-amzn-errortype` header, present only on an error answer.
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    /// The error shape this answer names, from the header or the body's `__type`.
    pub(crate) fn error_code(&self) -> String {
        self.error_type
            .clone()
            .or_else(|| {
                self.body
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    pub(crate) fn field(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    pub(crate) fn string(&self, name: &str) -> String {
        self.field(name)
            .as_str()
            .unwrap_or_else(|| panic!("response has no string member {name}: {}", self.body))
            .to_string()
    }
}

/// A router holding only this service, over one shared engine — what
/// `core/router/src/lib.rs` builds, minus every other service. The engine comes back
/// too, so a test can read the store directly where that is the only way to observe a
/// state transition without racing the clock.
///
/// The account is as a fresh one is: no uplink approval. Call
/// [`provision_uplink_approval`] for a suite that needs one.
pub(crate) fn harness_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Grant the account the uplink band [`UPLINK_APPROVAL_MHZ`] — what happens OUTSIDE this
/// API before Ground Station will create an antenna-uplink config
/// (`state::approved_uplink_bands_mhz`, which an operator sets through the environment
/// variable the crate README documents).
///
/// The suites here assert what a provisioned account can do; what an unprovisioned one
/// is answered instead is `tests/all/request_validation.rs`'s subject.
pub(crate) fn provision_uplink_approval(engine: &engine::InMemoryEngine) {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .approved_uplink_bands_mhz
        .push(UPLINK_APPROVAL_MHZ);
}

/// A router holding only this service, over one shared engine, on an account provisioned
/// by [`provision_uplink_approval`].
pub(crate) fn harness() -> MultiServiceRouter {
    let (router, engine) = harness_with_engine();
    provision_uplink_approval(&engine);
    router
}

/// Tell this account it is served by the ground stations `setting` describes — what
/// happens OUTSIDE this API, which models no operation that builds an inventory, so the
/// stations land exactly where `state::GROUND_STATIONS_ENV` lands them.
///
/// `pub(crate)`: a ground station named in a request is one of the stations this account
/// is served by, so every sibling suite that reserves antenna time or asks what is held
/// at a station says here which stations its account knows rather than relying on a
/// lookup that found none. Entries are ADDED to what the account already knows, so a
/// suite can stock a second station without losing the first.
pub(crate) fn stock_ground_stations(engine: &engine::InMemoryEngine, setting: &str) {
    let inventory = parse_ground_stations(setting);
    assert!(
        !inventory.is_empty(),
        "the setting {setting:?} must describe at least one ground station"
    );
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .ground_stations
        .extend(inventory);
}

/// The ground station the suites of this target book antenna time at by default.
///
/// `pub(crate)`: a reservation names a ground station this account is served by, so a
/// suite that books a pass stocks THIS station ([`stock_ground_stations`] /
/// [`harness_serving`]) and books at it, rather than each suite inventing a name of its
/// own beside its seeding call.
pub(crate) const STATION: &str = "Ohio-1";

/// A router over an account provisioned as [`harness`] does and served by the ground
/// stations `setting` describes ([`stock_ground_stations`]).
pub(crate) fn harness_serving(setting: &str) -> MultiServiceRouter {
    let (router, engine) = harness_with_engine();
    provision_uplink_approval(&engine);
    stock_ground_stations(&engine, setting);
    router
}

/// The WHOLE answer of one call — status, headers and raw body bytes — for a suite that
/// asserts on the FRAMING of an answer rather than on a parsed body (an answer that must
/// carry no body at all, for one). [`send`] is this call reduced to a [`Reply`].
pub(crate) async fn send_http(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> http::Response<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://groundstation.{REGION}.amazonaws.com{path}"))
        .header("host", format!("groundstation.{REGION}.amazonaws.com"))
        .header("authorization", AUTH);
    let payload = match &body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    router
        .dispatch(builder.body(payload).expect("build the request"))
        .await
}

pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let response = send_http(router, method, path, body).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// Every item a listing returns, across as many pages as it takes, in the order the
/// listing gave them.
///
/// `path` is the listing's route, `items_member` the response member that carries the
/// page, and `page_size` the caller's `maxResults` (`None` names none). Written once here
/// because every listing of this service pages the same way — one `nextToken`, one items
/// member — so a suite asserts its own listing rather than its own page walk.
pub(crate) async fn list_pages(
    router: &MultiServiceRouter,
    path: &str,
    items_member: &str,
    page_size: Option<i64>,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more page than a bounded store could need at a page size of one, so a token
    // that failed to advance is caught as a bounded failure instead of looping forever.
    for page in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let route = if query.is_empty() {
            path.to_string()
        } else {
            // A listing whose route already carries filters (a window, a type) keeps
            // them: the page size and the token join that query string rather than
            // starting a second one.
            let separator = if path.contains('?') { '&' } else { '?' };
            format!("{path}{separator}{}", query.join("&"))
        };

        let reply = send(router, "GET", &route, None).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={page_size:?} page-{page}: expected {path} 200, got {} {}",
            reply.status, reply.body
        );
        let items_on_page = reply
            .field(items_member)
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case maxResults={page_size:?} page-{page}: {items_member} must be a list, \
                     got {}",
                    reply.body
                )
            })
            .clone();
        if let Some(size) = page_size {
            assert!(
                items_on_page.len() as i64 <= size,
                "case maxResults={page_size:?} page-{page}: a page must hold at most {size} \
                 items, got {}",
                items_on_page.len()
            );
        }
        items.extend(items_on_page);

        token = reply.field("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            return items;
        }
    }
    panic!("case maxResults={page_size:?}: the listing of {path} never closed");
}

/// Whether `id` is spelled the way the modelled `Uuid` requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) — the shape of every
/// id this service mints, so the check is written once for every suite that asserts one.
pub(crate) fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
}

/// A spectral config with every optional member set, so a response that echoes it back
/// is comparable to the request verbatim (`polarization` carries a modelled default,
/// which an omitted member would materialise).
pub(crate) fn spectrum_config(center_mhz: f64, bandwidth_mhz: f64) -> Value {
    json!({
        "centerFrequency": {"value": center_mhz, "units": "MHz"},
        "bandwidth": {"value": bandwidth_mhz, "units": "MHz"},
        "polarization": "RIGHT_HAND",
    })
}

/// An `antennaUplinkConfig` body — also the config an `uplinkEchoConfig` must point at.
/// Its center frequency sits inside [`UPLINK_APPROVAL_MHZ`], the band this suite grants
/// the account.
pub(crate) fn antenna_uplink_config_data() -> Value {
    json!({
        "transmitDisabled": false,
        "spectrumConfig": {
            "centerFrequency": {"value": 2050.5, "units": "MHz"},
            "polarization": "LEFT_HAND",
        },
        "targetEirp": {"value": 20.5, "units": "dBW"},
    })
}

/// Every `(ConfigTypeData` member, resulting `ConfigCapabilityType)` pair.
///
/// Source: `smithy_query(query="shape", shape="ConfigTypeData")` for the union's eight
/// members and `shape="ConfigCapabilityType"` for the eight enum values — the mapping
/// under test, so the table lists all of them, not only the two the issue cited.
pub(crate) fn config_data_table(antenna_uplink_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "antenna-downlink",
            json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        ),
        (
            "antenna-downlink-demod-decode",
            json!({"antennaDownlinkDemodDecodeConfig": {
                "spectrumConfig": spectrum_config(7820.5, 40.5),
                "demodulationConfig": {"unvalidatedJSON": "{}"},
                "decodeConfig": {"unvalidatedJSON": "{}"},
            }}),
        ),
        (
            "antenna-uplink",
            json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        ),
        (
            "dataflow-endpoint",
            json!({"dataflowEndpointConfig": {
                "dataflowEndpointName": "downlink-endpoint",
                "dataflowEndpointRegion": REGION,
            }}),
        ),
        (
            "s3-recording",
            json!({"s3RecordingConfig": {
                "bucketArn": format!("arn:aws:s3:::{RECORDING_BUCKET}"),
                "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
                "prefix": "data/{satellite_id}/{year}/{month}/{day}",
            }}),
        ),
        (
            "telemetry-sink",
            json!({"telemetrySinkConfig": {
                "telemetrySinkType": "KINESIS_DATA_STREAM",
                "telemetrySinkData": {"kinesisDataStreamData": {
                    "kinesisRoleArn": "arn:aws:iam::000000000000:role/GroundStationTelemetryRole",
                    "kinesisDataStreamArn":
                        "arn:aws:kinesis:us-east-1:000000000000:stream/telemetry",
                }},
            }}),
        ),
        ("tracking", json!({"trackingConfig": {"autotrack": "PREFERRED"}})),
        (
            "uplink-echo",
            json!({"uplinkEchoConfig": {
                "enabled": true,
                "antennaUplinkConfigArn": antenna_uplink_arn,
            }}),
        ),
    ]
}

/// `CreateConfig`, asserting it succeeded, and answering `(configId, configType,
/// configArn)`.
pub(crate) async fn create_config(
    router: &MultiServiceRouter,
    name: &str,
    config_data: Value,
    tags: Option<Value>,
) -> (String, String, String) {
    let mut body = json!({"name": name, "configData": config_data});
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let reply = send(router, "POST", "/config", Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "CreateConfig {name}: expected 200, got {} {}",
        reply.status, reply.body
    );
    (
        reply.string("configId"),
        reply.string("configType"),
        reply.string("configArn"),
    )
}

/// A tracking config, the one every mission profile needs, plus its ARN.
pub(crate) async fn create_tracking_config(router: &MultiServiceRouter, name: &str) -> String {
    let (_, _, arn) = create_config(
        router,
        name,
        json!({"trackingConfig": {"autotrack": "REQUIRED"}}),
        None,
    )
    .await;
    arn
}

#[tokio::test]
async fn config_type_is_the_union_member_set_and_get_config_returns_what_was_created() {
    let router = harness();

    // `uplinkEchoConfig` references an antenna-uplink config, so that one exists first.
    let (_, _, antenna_uplink_arn) = create_config(
        &router,
        "echo-source-uplink",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    for (expected_type, config_data) in config_data_table(&antenna_uplink_arn) {
        let name = format!("profile-config {expected_type}");
        let tags = json!({"mission": "demo", "case": expected_type});
        let create = send(
            &router,
            "POST",
            "/config",
            Some(json!({"name": name, "configData": config_data, "tags": tags})),
        )
        .await;
        assert_eq!(
            create.status, 200,
            "case {expected_type}: expected CreateConfig 200, got {} {}",
            create.status, create.body
        );
        let config_id = create.string("configId");
        let config_type = create.string("configType");
        let config_arn = create.string("configArn");

        assert_eq!(
            config_type, expected_type,
            "case {expected_type}: configType must name the union member that was set, got {config_type:?}"
        );
        let expected_arn = format!(
            "arn:aws:groundstation:{REGION}:{}:config/{expected_type}/{config_id}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            config_arn, expected_arn,
            "case {expected_type}: configArn must address the stored config, got {config_arn:?}"
        );
        assert!(
            config_arn.len() >= 82,
            "case {expected_type}: configArn is shorter than the modelled minimum of 82, got {}",
            config_arn.len()
        );

        let got = send(
            &router,
            "GET",
            &format!("/config/{config_type}/{config_id}"),
            None,
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {expected_type}: expected GetConfig 200, got {} {}",
            got.status, got.body
        );
        assert_eq!(
            got.string("configId"),
            config_id,
            "case {expected_type}: GetConfig returned another configId"
        );
        assert_eq!(
            got.string("configArn"),
            config_arn,
            "case {expected_type}: GetConfig returned another configArn"
        );
        assert_eq!(
            got.string("name"),
            name,
            "case {expected_type}: GetConfig returned another name"
        );
        assert_eq!(
            got.field("configType"),
            &json!(expected_type),
            "case {expected_type}: GetConfig returned another configType"
        );
        assert_eq!(
            got.field("configData"),
            &config_data_table(&antenna_uplink_arn)
                .into_iter()
                .find(|(label, _)| *label == expected_type)
                .expect("the table row under test")
                .1,
            "case {expected_type}: GetConfig must return the configData that was created, got {}",
            got.field("configData")
        );
        assert_eq!(
            got.field("tags"),
            &tags,
            "case {expected_type}: GetConfig must return the tags that were created, got {}",
            got.field("tags")
        );

        // The pair of labels addresses a config: the same id under another type is not
        // this config, and is no config at all.
        let other_type = if expected_type == "tracking" {
            "antenna-downlink"
        } else {
            "tracking"
        };
        let mismatched = send(
            &router,
            "GET",
            &format!("/config/{other_type}/{config_id}"),
            None,
        )
        .await;
        assert_eq!(
            mismatched.status, 434,
            "case {expected_type}: GetConfig under type {other_type} must be 434, got {} {}",
            mismatched.status, mismatched.body
        );
        assert_eq!(
            mismatched.error_code(),
            "ResourceNotFoundException",
            "case {expected_type}: wrong error shape, got {}",
            mismatched.body
        );
    }

    let unknown = send(
        &router,
        "GET",
        &format!("/config/tracking/{ABSENT_UUID}"),
        None,
    )
    .await;
    assert_eq!(
        unknown.status, 434,
        "case unknown-config-id: expected 434, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case unknown-config-id: wrong error shape, got {}",
        unknown.body
    );
}

#[tokio::test]
async fn mission_profile_round_trips_every_member_it_was_created_with() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "profile-tracking").await;
    let (_, _, downlink_arn) = create_config(
        &router,
        "profile-downlink",
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7900.5, 20.5)}}),
        None,
    )
    .await;
    let (_, _, endpoint_arn) = create_config(
        &router,
        "profile-endpoint",
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": "downlink"}}),
        None,
    )
    .await;
    let (_, _, telemetry_arn) = create_config(
        &router,
        "profile-telemetry",
        json!({"telemetrySinkConfig": {
            "telemetrySinkType": "KINESIS_DATA_STREAM",
            "telemetrySinkData": {"kinesisDataStreamData": {
                "kinesisRoleArn": "arn:aws:iam::000000000000:role/GroundStationTelemetryRole",
                "kinesisDataStreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/telemetry",
            }},
        }}),
        None,
    )
    .await;
    // The far end of an UPLINK dataflow, so the two-edge row below carries one dataflow
    // in each direction rather than two of the same kind.
    let (_, _, uplink_arn) = create_config(
        &router,
        "profile-uplink",
        json!({"antennaUplinkConfig": antenna_uplink_config_data()}),
        None,
    )
    .await;

    // Rows differ along what the rule generalises over: the optional pre/post-pass
    // durations (absent, both published bounds), the number of dataflow edges, the
    // optional telemetry sink, tags, and the minimum viable duration (its published
    // bounds, from `smithy_query(query="shape", shape="PositiveDurationInSeconds")`:
    // range 1..21600).
    let rows: Vec<(&str, Value)> = vec![
        (
            "bare-minimum",
            json!({
                "name": "bare minimum",
                "minimumViableContactDurationSeconds": 1,
                "dataflowEdges": [],
                "trackingConfigArn": tracking_arn,
            }),
        ),
        (
            "one-edge-zero-durations",
            json!({
                "name": "one edge zero durations",
                "minimumViableContactDurationSeconds": 180,
                "contactPrePassDurationSeconds": 0,
                "contactPostPassDurationSeconds": 0,
                "dataflowEdges": [[downlink_arn, endpoint_arn]],
                "trackingConfigArn": tracking_arn,
                "tags": {"team": "ops"},
            }),
        ),
        (
            "two-edges-max-durations",
            json!({
                "name": "two edges max durations",
                "minimumViableContactDurationSeconds": 21600,
                "contactPrePassDurationSeconds": 21600,
                "contactPostPassDurationSeconds": 21600,
                "dataflowEdges": [
                    [downlink_arn, endpoint_arn],
                    [endpoint_arn, uplink_arn],
                ],
                "trackingConfigArn": tracking_arn,
                "telemetrySinkConfigArn": telemetry_arn,
                "tags": {"team": "ops", "mission": "demo"},
            }),
        ),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, request) in &rows {
        let create = send(&router, "POST", "/missionprofile", Some(request.clone())).await;
        assert_eq!(
            create.status, 200,
            "case {case}: expected CreateMissionProfile 200, got {} {}",
            create.status, create.body
        );
        let id = create.string("missionProfileId");
        created.push(id.clone());

        let got = send(&router, "GET", &format!("/missionprofile/{id}"), None).await;
        assert_eq!(
            got.status, 200,
            "case {case}: expected GetMissionProfile 200, got {} {}",
            got.status, got.body
        );
        assert_eq!(
            got.string("missionProfileId"),
            id,
            "case {case}: GetMissionProfile returned another id"
        );
        assert_eq!(
            got.string("missionProfileArn"),
            format!(
                "arn:aws:groundstation:{REGION}:{}:mission-profile/{id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case}: missionProfileArn must address the stored profile"
        );
        assert_eq!(
            got.field("region"),
            &json!(REGION),
            "case {case}: the profile's region must be the region it was created in"
        );
        // Every member the request carried must come back unchanged.
        for (member, sent) in request.as_object().expect("the request is an object") {
            assert_eq!(
                got.field(member),
                sent,
                "case {case}: member {member} must round-trip, got {}",
                got.field(member)
            );
        }
        // An absent optional member stays absent.
        if !request
            .as_object()
            .is_some_and(|row| row.contains_key("telemetrySinkConfigArn"))
        {
            assert!(
                got.field("telemetrySinkConfigArn").is_null(),
                "case {case}: telemetrySinkConfigArn was never set, got {}",
                got.field("telemetrySinkConfigArn")
            );
        }
    }

    // The listing holds exactly the profiles that were created.
    let list = send(&router, "GET", "/missionprofile", None).await;
    assert_eq!(
        list.status, 200,
        "case list-all: expected 200, got {} {}",
        list.status, list.body
    );
    let listed: Vec<String> = list
        .field("missionProfileList")
        .as_array()
        .expect("missionProfileList is a list")
        .iter()
        .map(|item| {
            item.get("missionProfileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let mut sorted_listed = listed.clone();
    sorted_listed.sort();
    let mut sorted_created = created.clone();
    sorted_created.sort();
    assert_eq!(
        sorted_listed, sorted_created,
        "case list-all: ListMissionProfiles must return every created profile, got {listed:?}"
    );

    let unknown = send(
        &router,
        "GET",
        &format!("/missionprofile/{ABSENT_UUID}"),
        None,
    )
    .await;
    assert_eq!(
        unknown.status, 434,
        "case unknown-profile: expected 434, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case unknown-profile: wrong error shape, got {}",
        unknown.body
    );
}

#[tokio::test]
async fn a_mission_profile_refuses_a_config_reference_that_resolves_to_nothing() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "reference-tracking").await;
    let (_, _, endpoint_arn) = create_config(
        &router,
        "reference-endpoint",
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": "downlink"}}),
        None,
    )
    .await;

    // One row per ARN-valued reference a profile carries: each must resolve.
    let rows: Vec<(&str, Value)> = vec![
        (
            "unknown-trackingConfigArn",
            json!({
                "name": "unknown tracking",
                "minimumViableContactDurationSeconds": 180,
                "dataflowEdges": [],
                "trackingConfigArn": ABSENT_CONFIG_ARN,
            }),
        ),
        (
            "unknown-edge-source",
            json!({
                "name": "unknown edge source",
                "minimumViableContactDurationSeconds": 180,
                "dataflowEdges": [[ABSENT_CONFIG_ARN, endpoint_arn]],
                "trackingConfigArn": tracking_arn,
            }),
        ),
        (
            "unknown-edge-destination",
            json!({
                "name": "unknown edge destination",
                "minimumViableContactDurationSeconds": 180,
                "dataflowEdges": [[endpoint_arn, ABSENT_CONFIG_ARN]],
                "trackingConfigArn": tracking_arn,
            }),
        ),
        (
            "unknown-telemetrySinkConfigArn",
            json!({
                "name": "unknown telemetry sink",
                "minimumViableContactDurationSeconds": 180,
                "dataflowEdges": [],
                "trackingConfigArn": tracking_arn,
                "telemetrySinkConfigArn": ABSENT_CONFIG_ARN,
            }),
        ),
        (
            // A reference whose member names ONE config type is refused by an ARN that
            // resolves to a config of another type, just as by one that resolves to
            // nothing: here a dataflow-endpoint config where a tracking config belongs.
            "trackingConfigArn-of-another-type",
            json!({
                "name": "tracking that is an endpoint",
                "minimumViableContactDurationSeconds": 180,
                "dataflowEdges": [],
                "trackingConfigArn": endpoint_arn,
            }),
        ),
        (
            "telemetrySinkConfigArn-of-another-type",
            json!({
                "name": "telemetry sink that is tracking",
                "minimumViableContactDurationSeconds": 180,
                "dataflowEdges": [],
                "trackingConfigArn": tracking_arn,
                "telemetrySinkConfigArn": tracking_arn,
            }),
        ),
    ];

    for (case, request) in rows {
        let reply = send(&router, "POST", "/missionprofile", Some(request)).await;
        assert_eq!(
            reply.status, 434,
            "case {case}: expected 434, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case}: wrong error shape, got {}",
            reply.body
        );
    }

    // And an `uplinkEchoConfig` pointing at no config is refused the same way.
    let echo = send(
        &router,
        "POST",
        "/config",
        Some(json!({
            "name": "echo of nothing",
            "configData": {"uplinkEchoConfig": {
                "enabled": true,
                "antennaUplinkConfigArn": ABSENT_CONFIG_ARN,
            }},
        })),
    )
    .await;
    assert_eq!(
        echo.status, 434,
        "case unknown-antennaUplinkConfigArn: expected 434, got {} {}",
        echo.status, echo.body
    );
    assert_eq!(
        echo.error_code(),
        "ResourceNotFoundException",
        "case unknown-antennaUplinkConfigArn: wrong error shape, got {}",
        echo.body
    );

    // ... while one pointing at a config that EXISTS but is of another type is NOT a
    // not-found: the ARN resolved, and what is wrong with it is the capability type it
    // names, which is a bad parameter. The whole class of those rows, and the text,
    // is `tests/all/config_defaults_and_references.rs`'s subject.
    let wrong_type = send(
        &router,
        "POST",
        "/config",
        Some(json!({
            "name": "echo of a tracking config",
            "configData": {"uplinkEchoConfig": {
                "enabled": true,
                "antennaUplinkConfigArn": tracking_arn,
            }},
        })),
    )
    .await;
    assert_eq!(
        wrong_type.status, 431,
        "case antennaUplinkConfigArn-of-another-type: expected 431, got {} {}",
        wrong_type.status, wrong_type.body
    );
    assert_eq!(
        wrong_type.error_code(),
        "InvalidParameterException",
        "case antennaUplinkConfigArn-of-another-type: wrong error shape, got {}",
        wrong_type.body
    );
}

#[tokio::test]
async fn list_mission_profiles_pages_through_every_profile_once() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "paging-tracking").await;

    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        let reply = send(
            &router,
            "POST",
            "/missionprofile",
            Some(json!({
                "name": format!("paging profile {index}"),
                "minimumViableContactDurationSeconds": 180,
                "dataflowEdges": [],
                "trackingConfigArn": tracking_arn,
            })),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case create-{index}: expected 200, got {} {}",
            reply.status, reply.body
        );
        created.push(reply.string("missionProfileId"));
    }
    created.sort();

    // One profile per page: three pages, then a page that closes the listing.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..3 {
        let path = match &token {
            None => "/missionprofile?maxResults=1".to_string(),
            Some(token) => format!("/missionprofile?maxResults=1&nextToken={token}"),
        };
        let reply = send(&router, "GET", &path, None).await;
        assert_eq!(
            reply.status, 200,
            "case page-{page}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let items = reply
            .field("missionProfileList")
            .as_array()
            .expect("missionProfileList is a list")
            .clone();
        assert_eq!(
            items.len(),
            1,
            "case page-{page}: maxResults=1 must return one item, got {}",
            items.len()
        );
        seen.push(
            items[0]
                .get("missionProfileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        token = reply
            .field("nextToken")
            .as_str()
            .map(str::to_string);
        if page < 2 {
            assert!(
                token.is_some(),
                "case page-{page}: a page with more to come must carry a nextToken, got {}",
                reply.body
            );
        }
    }
    assert!(
        token.is_none(),
        "case last-page: the final page must close the listing, got nextToken {token:?}"
    );
    assert_eq!(
        seen, created,
        "case paged-union: paging must yield every profile exactly once, got {seen:?}"
    );

    let bogus = send(
        &router,
        "GET",
        "/missionprofile?maxResults=1&nextToken=not-a-token",
        None,
    )
    .await;
    assert_eq!(
        bogus.status, 431,
        "case bogus-token: expected 431, got {} {}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error_code(),
        "InvalidParameterException",
        "case bogus-token: wrong error shape, got {}",
        bogus.body
    );

    // A token names the listing that issued it, so another listing refuses it rather
    // than resuming at an offset into an unrelated sequence.
    let first_page = send(&router, "GET", "/missionprofile?maxResults=1", None).await;
    let profiles_token = first_page
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page hands back a token");
    let crossed = send(
        &router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": ["SCHEDULED"],
            "startTime": 1_800_000_000_i64,
            "endTime": 1_800_003_600_i64,
            "nextToken": profiles_token,
        })),
    )
    .await;
    assert_eq!(
        crossed.status, 431,
        "case token-from-another-listing: expected 431, got {} {}",
        crossed.status, crossed.body
    );
    assert_eq!(
        crossed.error_code(),
        "InvalidParameterException",
        "case token-from-another-listing: wrong error shape, got {}",
        crossed.body
    );
}

/// Create a mission profile and answer its ARN.
pub(crate) async fn create_mission_profile(
    router: &MultiServiceRouter,
    name: &str,
    minimum_seconds: i64,
    pre_pass: Option<i64>,
    post_pass: Option<i64>,
) -> String {
    let tracking_arn = create_tracking_config(router, &format!("{name} tracking")).await;
    let mut body = json!({
        "name": name,
        "minimumViableContactDurationSeconds": minimum_seconds,
        "dataflowEdges": [],
        "trackingConfigArn": tracking_arn,
    });
    if let Some(pre) = pre_pass {
        body["contactPrePassDurationSeconds"] = json!(pre);
    }
    if let Some(post) = post_pass {
        body["contactPostPassDurationSeconds"] = json!(post);
    }
    let created = send(router, "POST", "/missionprofile", Some(body)).await;
    assert_eq!(
        created.status, 200,
        "CreateMissionProfile {name}: expected 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("missionProfileId");
    let got = send(router, "GET", &format!("/missionprofile/{id}"), None).await;
    got.string("missionProfileArn")
}

#[tokio::test]
async fn a_reserved_contact_settles_into_scheduled_and_reads_back_from_state() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "lifecycle", 180, Some(120), Some(60)).await;

    let start = 1_800_000_000_i64;
    let end = start + 600;
    // A contact is flown against a satellite of THIS account, so the reservation names an
    // on-boarded one (`crate::contact_satellite_reference_integrity`): an ARN no onboarding
    // carries references nothing and is refused.
    let satellite_arn =
        crate::contact_satellite_reference_integrity::onboard_satellite_arns(&engine, 1)[0]
            .clone();
    let reserve = send(
        &router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": "Ohio-1",
            "startTime": start,
            "endTime": end,
            "satelliteArn": satellite_arn,
            "tags": {"mission": "demo"},
        })),
    )
    .await;
    assert_eq!(
        reserve.status, 200,
        "case reserve: expected 200, got {} {}",
        reserve.status, reserve.body
    );
    let contact_id = reserve.string("contactId");
    assert_eq!(
        reserve.field("versionId"),
        &json!(1),
        "case reserve: the first version of a contact is 1, got {}",
        reserve.field("versionId")
    );

    // A reservation is admitted in SCHEDULING, with the move to SCHEDULED owed by the
    // clock. Read straight from the store, because only the store shows the admitted
    // state without racing that span: nothing settles a contact except a later read.
    {
        let state = service_state(&*engine).expect("this service's store");
        let guard = state.lock();
        let contact = guard
            .contacts
            .get(&contact_id)
            .expect("the reserved contact is in the store");
        assert_eq!(
            contact.contact_status, "SCHEDULING",
            "case admitted: a reservation is admitted in SCHEDULING, got {:?}",
            contact.contact_status
        );
        assert!(
            contact.scheduling_settles_at.is_some(),
            "case admitted: the scheduler must owe this contact its move to SCHEDULED"
        );
    }

    // Over HTTP the same read is one of the two statuses, depending only on whether the
    // owed move already came due — never anything else.
    let immediate = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        immediate.status, 200,
        "case describe-immediately: expected 200, got {} {}",
        immediate.status, immediate.body
    );
    let immediate_status = immediate.string("contactStatus");
    assert!(
        immediate_status == "SCHEDULING" || immediate_status == "SCHEDULED",
        "case describe-immediately: expected SCHEDULING or SCHEDULED, got {immediate_status:?}"
    );

    std::thread::sleep(AFTER_SETTLE);

    let described = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case describe-after-settle: expected 200, got {} {}",
        described.status, described.body
    );
    assert_eq!(
        described.field("contactStatus"),
        &json!("SCHEDULED"),
        "case describe-after-settle: the scheduler owed SCHEDULED, got {}",
        described.field("contactStatus")
    );
    assert_eq!(
        described.string("contactId"),
        contact_id,
        "case describe-after-settle: another contactId came back"
    );
    assert_eq!(
        described.string("missionProfileArn"),
        profile_arn,
        "case describe-after-settle: another missionProfileArn came back"
    );
    assert_eq!(
        described.string("satelliteArn"),
        satellite_arn,
        "case describe-after-settle: another satelliteArn came back"
    );
    assert_eq!(
        described.string("groundStation"),
        "Ohio-1",
        "case describe-after-settle: another groundStation came back"
    );
    assert_eq!(
        described.field("region"),
        &json!(REGION),
        "case describe-after-settle: the contact's region must be where it was reserved"
    );
    assert_eq!(
        described.field("tags"),
        &json!({"mission": "demo"}),
        "case describe-after-settle: the tags that were set must come back, got {}",
        described.field("tags")
    );
    for (member, expected) in [
        ("startTime", start as f64),
        ("endTime", end as f64),
        // The profile's pre/post-pass durations widen the window: 120 s before and
        // 60 s after.
        ("prePassStartTime", (start - 120) as f64),
        ("postPassEndTime", (end + 60) as f64),
    ] {
        let actual = described.field(member).as_f64();
        assert_eq!(
            actual,
            Some(expected),
            "case describe-after-settle: {member} must be {expected}, got {actual:?}"
        );
    }
    assert_eq!(
        described
            .field("version")
            .get("versionId")
            .and_then(Value::as_i64),
        Some(1),
        "case describe-after-settle: the contact's version record must be version 1, got {}",
        described.field("version")
    );

    // Cancelling is visible to the next reader, and takes the contact out of the
    // SCHEDULED listing.
    let before_cancel = list_contacts(&router, json!({
        "statusList": ["SCHEDULED"],
        "startTime": start - 3600,
        "endTime": end + 3600,
    }))
    .await;
    assert_eq!(
        before_cancel,
        vec![contact_id.clone()],
        "case list-before-cancel: the scheduled contact must be listed, got {before_cancel:?}"
    );

    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case cancel: expected 200, got {} {}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        cancelled.string("contactId"),
        contact_id,
        "case cancel: another contactId came back"
    );
    assert_eq!(
        cancelled.field("versionId"),
        &json!(1),
        "case cancel: expected versionId 1, got {}",
        cancelled.field("versionId")
    );

    let after_cancel = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        after_cancel.field("contactStatus"),
        &json!("CANCELLED"),
        "case describe-after-cancel: expected CANCELLED, got {}",
        after_cancel.field("contactStatus")
    );
    let still_scheduled = list_contacts(&router, json!({
        "statusList": ["SCHEDULED"],
        "startTime": start - 3600,
        "endTime": end + 3600,
    }))
    .await;
    assert!(
        still_scheduled.is_empty(),
        "case list-after-cancel: a cancelled contact is not SCHEDULED, got {still_scheduled:?}"
    );
    let now_cancelled = list_contacts(&router, json!({
        "statusList": ["CANCELLED"],
        "startTime": start - 3600,
        "endTime": end + 3600,
    }))
    .await;
    assert_eq!(
        now_cancelled,
        vec![contact_id.clone()],
        "case list-cancelled: the contact must list under its new status, got {now_cancelled:?}"
    );

    // @idempotent, but a cancel RELEASES a reservation: a contact that no longer holds
    // one has nothing left to cancel, so the second DELETE is refused with the
    // operation's modelled InvalidParameterException instead of answering 200.
    let again = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        again.status, 431,
        "case cancel-again: expected the modelled InvalidParameterException status 431, \
         got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "InvalidParameterException",
        "case cancel-again: wrong error shape, got {}",
        again.body
    );
    let still_cancelled = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        still_cancelled.field("contactStatus"),
        &json!("CANCELLED"),
        "case cancel-again: a refused cancel leaves the status untouched, got {}",
        still_cancelled.field("contactStatus")
    );

    for (case, path) in [
        ("describe", format!("/contact/{ABSENT_UUID}")),
        ("cancel", format!("/contact/{ABSENT_UUID}")),
    ] {
        let method = if case == "describe" { "GET" } else { "DELETE" };
        let reply = send(&router, method, &path, None).await;
        assert_eq!(
            reply.status, 434,
            "case unknown-contact-{case}: expected 434, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case unknown-contact-{case}: wrong error shape, got {}",
            reply.body
        );
    }
}

/// `ListContacts` with `filters`, answering the ids it returned in order.
async fn list_contacts(router: &MultiServiceRouter, filters: Value) -> Vec<String> {
    let reply = send(router, "POST", "/contacts", Some(filters.clone())).await;
    assert_eq!(
        reply.status, 200,
        "ListContacts {filters}: expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactList")
        .as_array()
        .expect("contactList is a list")
        .iter()
        .map(|item| {
            item.get("contactId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn reserve_contact_enforces_the_window_the_mission_profile_requires() {
    // The account is served by the station every row reserves at: a reservation names a
    // ground station this account knows.
    let router = harness_serving("Ohio-1");
    let minimum = 180_i64;
    let profile_arn = create_mission_profile(&router, "windows", minimum, None, None).await;
    let start = 1_800_000_000_i64;

    // Rows differ along the window's length relative to the profile's minimum viable
    // contact duration, including both sides of that bound and a window that runs
    // backwards. Each row books a day of its own, because an antenna serves one pass at a
    // time and the accepted rows would otherwise be two passes over one window.
    let rows: Vec<(&str, i64, u16)> = vec![
        ("window-runs-backwards", -1, 431),
        ("empty-window", 0, 431),
        ("one-second-short", minimum - 1, 431),
        ("exactly-the-minimum", minimum, 200),
        ("longer-than-the-minimum", minimum * 4, 200),
    ];

    for (slot, (case, length, expected_status)) in rows.into_iter().enumerate() {
        let start = start + i64::try_from(slot).expect("a row index fits in i64") * 86_400;
        let reply = send(
            &router,
            "POST",
            "/contact",
            Some(json!({
                "missionProfileArn": profile_arn,
                "groundStation": "Ohio-1",
                "startTime": start,
                "endTime": start + length,
            })),
        )
        .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case}: a {length}s window must answer {expected_status}, got {} {}",
            reply.status, reply.body
        );
        if expected_status == 431 {
            assert_eq!(
                reply.error_code(),
                "InvalidParameterException",
                "case {case}: wrong error shape, got {}",
                reply.body
            );
        } else {
            assert!(
                !reply.string("contactId").is_empty(),
                "case {case}: an accepted reservation must carry a contactId, got {}",
                reply.body
            );
        }
    }

    let unknown_profile = send(
        &router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": ABSENT_MISSION_PROFILE_ARN,
            "groundStation": "Ohio-1",
            "startTime": start,
            "endTime": start + 600,
        })),
    )
    .await;
    assert_eq!(
        unknown_profile.status, 434,
        "case unknown-mission-profile: expected 434, got {} {}",
        unknown_profile.status, unknown_profile.body
    );
    assert_eq!(
        unknown_profile.error_code(),
        "ResourceNotFoundException",
        "case unknown-mission-profile: wrong error shape, got {}",
        unknown_profile.body
    );
}

#[tokio::test]
async fn list_contacts_returns_exactly_the_contacts_its_filters_select() {
    let (router, engine) = harness_with_engine();
    let profile_a = create_mission_profile(&router, "filters a", 60, None, None).await;
    let profile_b = create_mission_profile(&router, "filters b", 60, None, None).await;
    // The one contact of the table below that names a satellite is flown against an
    // on-boarded one; the availability rows further down filter on the same ARN.
    let satellite_arn =
        crate::contact_satellite_reference_integrity::onboard_satellite_arns(&engine, 1)[0]
            .clone();
    let start = 1_800_000_000_i64;

    // Three contacts differing in ground station, mission profile, satellite and
    // window, so each filter has something to exclude.
    let mut ids = Vec::new();
    for (profile, ground_station, offset, satellite) in [
        (&profile_a, "Ohio-1", 0_i64, Some(satellite_arn.clone())),
        (&profile_a, "Dubbo-1", 7_200, None),
        (&profile_b, "Ohio-1", 86_400, None),
    ] {
        let mut body = json!({
            "missionProfileArn": profile,
            "groundStation": ground_station,
            "startTime": start + offset,
            "endTime": start + offset + 600,
        });
        if let Some(satellite) = satellite {
            body["satelliteArn"] = json!(satellite);
        }
        let reply = send(&router, "POST", "/contact", Some(body)).await;
        assert_eq!(
            reply.status, 200,
            "case reserve-{ground_station}-{offset}: expected 200, got {} {}",
            reply.status, reply.body
        );
        ids.push(reply.string("contactId"));
    }
    std::thread::sleep(AFTER_SETTLE);

    let all_statuses = json!(["SCHEDULING", "SCHEDULED", "CANCELLED"]);
    let rows: Vec<(&str, Value, Vec<usize>)> = vec![
        (
            "whole-window",
            json!({
                "statusList": all_statuses,
                "startTime": start - 3600,
                "endTime": start + 90_000,
            }),
            vec![0, 1, 2],
        ),
        (
            "window-excludes-the-last",
            json!({
                "statusList": all_statuses,
                "startTime": start - 3600,
                "endTime": start + 10_000,
            }),
            vec![0, 1],
        ),
        (
            "by-ground-station",
            json!({
                "statusList": all_statuses,
                "startTime": start - 3600,
                "endTime": start + 90_000,
                "groundStation": "Dubbo-1",
            }),
            vec![1],
        ),
        (
            "by-mission-profile",
            json!({
                "statusList": all_statuses,
                "startTime": start - 3600,
                "endTime": start + 90_000,
                "missionProfileArn": profile_b,
            }),
            vec![2],
        ),
        (
            "by-satellite",
            json!({
                "statusList": all_statuses,
                "startTime": start - 3600,
                "endTime": start + 90_000,
                "satelliteArn": satellite_arn,
            }),
            vec![0],
        ),
        (
            "status-that-matches-nothing",
            json!({
                "statusList": ["FAILED"],
                "startTime": start - 3600,
                "endTime": start + 90_000,
            }),
            vec![],
        ),
    ];

    for (case, filters, expected_indexes) in rows {
        let returned = list_contacts(&router, filters).await;
        let mut expected: Vec<String> = expected_indexes
            .iter()
            .map(|index| ids[*index].clone())
            .collect();
        expected.sort();
        let mut actual = returned.clone();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case}: ListContacts must return exactly the selected contacts, got {returned:?}"
        );
    }

    // The `ephemeris` member NARROWS the listing: one naming an ephemeris that points no
    // contact of this account selects nothing, which is an answered request with an empty
    // page rather than a refusal. What each ephemeris filter DOES select is
    // `crate::available_contacts`'s subject.
    let by_ephemeris = send(
        &router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": all_statuses,
            "startTime": start - 3600,
            "endTime": start + 90_000,
            "ephemeris": {"azEl": {"id": "33333333-3333-3333-3333-333333333333"}},
        })),
    )
    .await;
    assert_eq!(
        by_ephemeris.status, 200,
        "case by-ephemeris: expected 200, got {} {}",
        by_ephemeris.status, by_ephemeris.body
    );
    assert_eq!(
        by_ephemeris.field("contactList").as_array(),
        Some(&Vec::new()),
        "case by-ephemeris: a filter nothing is pointed by must select nothing, got {}",
        by_ephemeris.body
    );

    // A request the operation's own documentation declares malformed is refused, naming
    // the member at fault: the window must run forwards, and an AVAILABLE search must
    // name the ground station, mission profile and satellite it is an availability
    // search for ("If statusList contains AVAILABLE, the request must include
    // groundStation, missionprofileArn, and satelliteArn" — the ListContacts doc).
    let satellite_filter = satellite_arn.clone();
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "window-runs-backwards",
            json!({
                "statusList": all_statuses,
                "startTime": start + 90_000,
                "endTime": start - 3600,
            }),
            "endTime",
        ),
        (
            "available-without-groundStation",
            json!({
                "statusList": ["AVAILABLE"],
                "startTime": start - 3600,
                "endTime": start + 90_000,
                "missionProfileArn": profile_a,
                "satelliteArn": satellite_filter,
            }),
            "groundStation",
        ),
        (
            "available-without-missionProfileArn",
            json!({
                "statusList": ["AVAILABLE"],
                "startTime": start - 3600,
                "endTime": start + 90_000,
                "groundStation": "Ohio-1",
                "satelliteArn": satellite_filter,
            }),
            "missionProfileArn",
        ),
        (
            "available-without-satelliteArn",
            json!({
                "statusList": ["AVAILABLE"],
                "startTime": start - 3600,
                "endTime": start + 90_000,
                "groundStation": "Ohio-1",
                "missionProfileArn": profile_a,
            }),
            "satelliteArn",
        ),
    ];
    for (case, request, parameter) in refusals {
        let refused = send(&router, "POST", "/contacts", Some(request)).await;
        assert_eq!(
            refused.status, 431,
            "case {case}: expected 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {case}: wrong error shape, got {}",
            refused.body
        );
        assert_eq!(
            refused.field("parameterName").as_str(),
            Some(parameter),
            "case {case}: the refusal must name the member at fault, got {}",
            refused.body
        );
    }

    // An AVAILABLE search that names all three is a well-formed request; no pointing data
    // is in force for the pairing it names — this account holds no ephemeris here, so the
    // satellite it is on-boarded to has no trajectory — so that pairing can fly nothing and
    // no window is offered. The windows a pairing that HAS pointing data is offered are
    // `crate::available_contacts`'s subject.
    let available = send(
        &router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": ["AVAILABLE"],
            "startTime": start - 3600,
            "endTime": start + 90_000,
            "groundStation": "Ohio-1",
            "missionProfileArn": profile_a,
            "satelliteArn": satellite_filter,
        })),
    )
    .await;
    assert_eq!(
        available.status, 200,
        "case available-with-every-member: expected 200, got {} {}",
        available.status, available.body
    );
    assert_eq!(
        available.field("contactList").as_array(),
        Some(&Vec::new()),
        "case available-with-every-member: a pairing with no pointing data in force offers \
         no window, got {}",
        available.body
    );

    // maxResults pages the same selection, and the token round-trips.
    let first = send(
        &router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": all_statuses,
            "startTime": start - 3600,
            "endTime": start + 90_000,
            "maxResults": 1,
        })),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case page-1: expected 200, got {} {}",
        first.status, first.body
    );
    let first_page = first
        .field("contactList")
        .as_array()
        .expect("contactList is a list")
        .clone();
    assert_eq!(
        first_page.len(),
        1,
        "case page-1: maxResults=1 must return one contact, got {}",
        first_page.len()
    );
    let token = first
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a listing with more to come carries a nextToken");
    let second = send(
        &router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": all_statuses,
            "startTime": start - 3600,
            "endTime": start + 90_000,
            "maxResults": 1,
            "nextToken": token,
        })),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "case page-2: expected 200, got {} {}",
        second.status, second.body
    );
    let second_page = second
        .field("contactList")
        .as_array()
        .expect("contactList is a list")
        .clone();
    assert_eq!(
        second_page.len(),
        1,
        "case page-2: maxResults=1 must return one contact, got {}",
        second_page.len()
    );
    assert_ne!(
        first_page[0].get("contactId"),
        second_page[0].get("contactId"),
        "case page-2: the second page must resume after the first, got {}",
        second.body
    );

    // An offset counts within the filtered sequence, so the same token under different
    // filters is refused rather than resuming at an offset into a different sequence.
    let narrowed = send(
        &router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": all_statuses,
            "startTime": start - 3600,
            "endTime": start + 90_000,
            "groundStation": "Dubbo-1",
            "maxResults": 1,
            "nextToken": token,
        })),
    )
    .await;
    assert_eq!(
        narrowed.status, 431,
        "case token-under-other-filters: expected 431, got {} {}",
        narrowed.status, narrowed.body
    );
    assert_eq!(
        narrowed.error_code(),
        "InvalidParameterException",
        "case token-under-other-filters: wrong error shape, got {}",
        narrowed.body
    );
}

#[tokio::test]
async fn every_minted_id_is_a_distinct_version_4_uuid() {
    let router = harness();

    // Every id member of this service targets the model's `Uuid` shape
    // (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, from
    // `smithy_query(query="shape", shape="Uuid")`), in the version-4 layout. Minting many
    // in a row — fast enough that a clock reading alone would repeat — is what shows the
    // ids are unique per mint and not per instant.
    const MINTS: usize = 64;
    let mut ids = Vec::with_capacity(MINTS);
    for index in 0..MINTS {
        let (config_id, _, _) = create_config(
            &router,
            &format!("minted {index}"),
            json!({"trackingConfig": {"autotrack": "PREFERRED"}}),
            None,
        )
        .await;
        ids.push(config_id);
    }

    for id in &ids {
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case {id}: a uuid is five hyphen-separated hex groups of 8-4-4-4-12"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.bytes().all(|b| b.is_ascii_digit()
                    || (b'a'..=b'f').contains(&b))),
            "case {id}: every character must be lowercase hex"
        );
        assert_eq!(
            groups[2].as_bytes()[0] as char,
            '4',
            "case {id}: the version nibble of a version-4 uuid is 4"
        );
        assert!(
            matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "case {id}: the variant bits of a version-4 uuid are 10, got {}",
            groups[3]
        );
    }

    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "{MINTS} mints must give {MINTS} distinct ids, got {} distinct",
        unique.len()
    );
}

#[tokio::test]
async fn a_listing_is_bounded_to_one_page_and_a_full_store_refuses_the_next_create() {
    let router = harness();
    let tracking_arn = create_tracking_config(&router, "bounds-tracking").await;

    // One more profile than a page holds, so the answer to a request that names no
    // page size cannot be "the whole store".
    for index in 0..=MAX_PAGE_SIZE {
        let reply = send(
            &router,
            "POST",
            "/missionprofile",
            Some(json!({
                "name": format!("bounds profile {index}"),
                "minimumViableContactDurationSeconds": 60,
                "dataflowEdges": [],
                "trackingConfigArn": tracking_arn,
            })),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case create-{index}: expected 200, got {} {}",
            reply.status, reply.body
        );
    }

    let unbounded_request = send(&router, "GET", "/missionprofile", None).await;
    assert_eq!(
        unbounded_request.status, 200,
        "case no-maxResults: expected 200, got {} {}",
        unbounded_request.status, unbounded_request.body
    );
    let page = unbounded_request
        .field("missionProfileList")
        .as_array()
        .expect("missionProfileList is a list")
        .len();
    assert_eq!(
        page, MAX_PAGE_SIZE,
        "case no-maxResults: a listing is capped at {MAX_PAGE_SIZE} items per page, got {page}"
    );
    let token = unbounded_request
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page must hand back a token for the rest");
    let rest = send(
        &router,
        "GET",
        &format!("/missionprofile?nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        rest.field("missionProfileList")
            .as_array()
            .expect("missionProfileList is a list")
            .len(),
        1,
        "case second-page: the remaining profile must come back, got {}",
        rest.body
    );
    assert!(
        rest.field("nextToken").is_null(),
        "case second-page: the final page must close the listing, got {}",
        rest.field("nextToken")
    );

    // A store at its bound refuses the next create with the service's own
    // account-limit error rather than growing without end. The config store holds one
    // record so far (the tracking config above), so this fills it to the bound.
    for index in 1..MAX_RECORDS_PER_STORE {
        let reply = send(
            &router,
            "POST",
            "/config",
            Some(json!({
                "name": format!("filler config {index}"),
                "configData": {"trackingConfig": {"autotrack": "PREFERRED"}},
            })),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case filler-{index}: expected 200, got {} {}",
            reply.status, reply.body
        );
    }
    let over_the_bound = send(
        &router,
        "POST",
        "/config",
        Some(json!({
            "name": "one config too many",
            "configData": {"trackingConfig": {"autotrack": "PREFERRED"}},
        })),
    )
    .await;
    assert_eq!(
        over_the_bound.status, 429,
        "case over-the-bound: expected 429, got {} {}",
        over_the_bound.status, over_the_bound.body
    );
    assert_eq!(
        over_the_bound.error_code(),
        "ResourceLimitExceededException",
        "case over-the-bound: wrong error shape, got {}",
        over_the_bound.body
    );
}
