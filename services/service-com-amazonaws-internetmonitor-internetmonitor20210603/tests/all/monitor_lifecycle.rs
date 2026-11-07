// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the monitor lifecycle: `CreateMonitor`, `GetMonitor`,
//! `ListMonitors`, `UpdateMonitor`, `DeleteMonitor`, `TagResource` and
//! `ListTagsForResource`. `UntagResource` has its own suite in
//! `tests/all/untag_resource.rs`, which reuses this module's harness.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator binary
//! mounts, so the wire protocol (`restJson1`), the routing table, the boundary validator
//! and the handlers are all exercised the way a client exercises them.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::state::{
    service_state, CLIENT_TOKEN_RETENTION, MAX_CLIENT_TOKENS, MAX_MONITORS,
    MAX_RESOURCES_PER_MONITOR, MAX_TAGS_PER_MONITOR, MONITOR_ACTIVATION_DELAY,
    MONITOR_STATE_ACTIVE, MONITOR_STATE_INACTIVE, MONITOR_STATE_PENDING,
};
use service_com_amazonaws_internetmonitor_internetmonitor20210603::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case uses unless it is specifically about the region in an ARN.
///
/// This module owns the crate's test harness (`emulator`, `call`, `label`,
/// `create_monitor`, `expected_arn` and this constant); a sibling suite reuses it
/// through `crate::monitor_lifecycle::…` rather than writing a second copy.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Long enough to be past [`MONITOR_ACTIVATION_DELAY`] without being a visible pause.
fn past_activation() -> Duration {
    MONITOR_ACTIVATION_DELAY + Duration::from_millis(250)
}

/// One emulator: the shared engine behind the one service under test, mounted in the
/// production router.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] on an engine the caller holds, for a suite that also reads or seeds this
/// service's own store through `service_state(&*engine)`.
pub(crate) fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service and `region`.
/// It is what routes the request and what the handlers read the region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/internetmonitor/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// Percent-encode a value for an `@httpLabel` path segment, the way an AWS SDK encodes an
/// ARN into `/tags/{ResourceArn}`: every byte outside RFC 3986's unreserved set
/// (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) is escaped, `/` and `:` included.
///
/// Written out here rather than pulled from `percent-encoding`: this is the whole of the
/// crate's need for it, and a new dev-dependency adds an entry to the shared workspace
/// lockfile, which every fix landing in parallel then has to merge.
pub(crate) fn label(value: &str) -> String {
    /// The uppercase hex digits a percent-escape is written with.
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// Call the emulator, returning the HTTP status and the parsed JSON body (`Value::Null`
/// for an empty body).
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => Bytes::new(),
    };
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!(
            "https://internetmonitor.{region}.amazonaws.com{path}"
        ))
        .header("host", format!("internetmonitor.{region}.amazonaws.com"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// [`call_in`] in [`REGION`].
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, path, body).await
}

/// The ARN `CreateMonitor` must mint for `name` in `region`.
pub(crate) fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:internetmonitor:{region}:{ACCOUNT}:monitor/{name}")
}

/// A member's value, treating "absent" and "null" alike — the JSON codec omits an absent
/// optional member rather than writing `null`.
pub(crate) fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|found| !found.is_null())
}

/// The error code a refusal carries: the `__type` of the JSON error envelope, with any
/// namespace prefix dropped, which is what an SDK branches on. One rule for every suite
/// that reads a refusal, rather than one copy per suite.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` a refusal carries; see [`error_code`] for why it lives here.
pub(crate) fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// `Resources` as a set, so a case can assert set semantics without depending on order.
fn resource_set(value: &Value, field: &str) -> BTreeSet<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A wire timestamp as epoch seconds, for ordering two of them.
///
/// The vendored codegen does not carry `@timestampFormat` into a shape's schema, so every
/// timestamp this emulator writes takes the protocol default — epoch seconds for
/// `restJson1`. Both forms are accepted here so this helper keeps working if that
/// framework-level gap is closed.
pub(crate) fn timestamp_secs(value: &Value, field: &str) -> f64 {
    let found = value
        .get(field)
        .unwrap_or_else(|| panic!("{field} must be present, got {value}"));
    if let Some(seconds) = found.as_f64() {
        return seconds;
    }
    let text = found
        .as_str()
        .unwrap_or_else(|| panic!("{field} must be a timestamp, got {value}"));
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("{field} {text:?} is not a date-time: {error}"))
        .as_secs_f64()
}

/// Create a monitor, asserting the create succeeded, and hand back its ARN.
pub(crate) async fn create_monitor(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, "POST", "/v20210603/Monitors", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "create {body}: expected 200, got {status} with {created}"
    );
    created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("create {body}: no Arn in {created}"))
        .to_string()
}

// ---------------------------------------------------------------------------------------
// CreateMonitor -> GetMonitor round trip
// ---------------------------------------------------------------------------------------

/// One create request and what `GetMonitor` must then report for it.
struct CreateCase {
    /// Names the row in every assertion message.
    case: &'static str,
    /// The `MonitorName` the row creates.
    name: String,
    /// The full `CreateMonitor` body.
    body: Value,
    /// `Resources` the monitor must end up with (the request's list as a set).
    resources: BTreeSet<String>,
}

/// The optional members whose stored value `GetMonitor` must report verbatim. Each is
/// optional on both sides, so "absent in the request" must read back as "absent".
const VERBATIM_MEMBERS: &[&str] = &[
    "MaxCityNetworksToMonitor",
    "TrafficPercentageToMonitor",
    "InternetMeasurementsLogDelivery",
];

fn create_cases() -> Vec<CreateCase> {
    let long_name = "l".repeat(255);
    vec![
        CreateCase {
            case: "no optional member at all",
            name: "bare-monitor".to_string(),
            body: json!({ "MonitorName": "bare-monitor" }),
            resources: BTreeSet::new(),
        },
        CreateCase {
            case: "one resource, both scalars at their minimum",
            name: "min-monitor".to_string(),
            body: json!({
                "MonitorName": "min-monitor",
                "Resources": ["arn:aws:ec2:us-east-1:000000000000:vpc/vpc-aaaa1111"],
                "MaxCityNetworksToMonitor": 1,
                "TrafficPercentageToMonitor": 1,
            }),
            resources: ["arn:aws:ec2:us-east-1:000000000000:vpc/vpc-aaaa1111".to_string()]
                .into_iter()
                .collect(),
        },
        CreateCase {
            case: "both scalars at their maximum, two tags",
            name: "max.monitor".to_string(),
            body: json!({
                "MonitorName": "max.monitor",
                "Resources": ["arn:aws:cloudfront::000000000000:distribution/E1EXAMPLE"],
                "MaxCityNetworksToMonitor": 500_000,
                "TrafficPercentageToMonitor": 100,
                "Tags": { "Env": "dev", "Team": "net" },
            }),
            resources: ["arn:aws:cloudfront::000000000000:distribution/E1EXAMPLE".to_string()]
                .into_iter()
                .collect(),
        },
        CreateCase {
            case: "three resources with one duplicate: SetOfARNs collapses it",
            name: "set_monitor".to_string(),
            body: json!({
                "MonitorName": "set_monitor",
                "Resources": [
                    "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-bbbb2222",
                    "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-cccc3333",
                    "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-bbbb2222",
                ],
            }),
            resources: [
                "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-bbbb2222".to_string(),
                "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-cccc3333".to_string(),
            ]
            .into_iter()
            .collect(),
        },
        CreateCase {
            case: "structure members: log delivery and health events config",
            name: "structured-monitor".to_string(),
            body: json!({
                "MonitorName": "structured-monitor",
                "Resources": ["arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/nlb/abc"],
                "InternetMeasurementsLogDelivery": {
                    "S3Config": {
                        "BucketName": "measurements-bucket",
                        "BucketPrefix": "internetmonitor/",
                        "LogDeliveryStatus": "ENABLED",
                    }
                },
                "HealthEventsConfig": {
                    "AvailabilityScoreThreshold": 97.5,
                    "PerformanceScoreThreshold": 92.25,
                },
                "Tags": { "Env": "prod" },
            }),
            resources: [
                "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/nlb/abc"
                    .to_string(),
            ]
            .into_iter()
            .collect(),
        },
        CreateCase {
            case: "a name at the published 255-character maximum",
            name: long_name.clone(),
            body: json!({ "MonitorName": long_name }),
            resources: BTreeSet::new(),
        },
        CreateCase {
            case: "a name at the published 1-character minimum",
            name: "a".to_string(),
            body: json!({ "MonitorName": "a" }),
            resources: BTreeSet::new(),
        },
    ]
}

#[tokio::test]
async fn get_monitor_reports_exactly_what_create_monitor_stored() {
    let router = emulator();

    for case in create_cases() {
        let arn = create_monitor(&router, case.body.clone()).await;
        assert_eq!(
            arn,
            expected_arn(REGION, &case.name),
            "case {}: unexpected Arn {arn}",
            case.case
        );

        let (status, got) = call(
            &router,
            "GET",
            &format!("/v20210603/Monitors/{}", label(&case.name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: GetMonitor expected 200, got {status} with {got}",
            case.case
        );

        assert_eq!(
            got["MonitorName"], json!(case.name),
            "case {}: unexpected MonitorName in {got}",
            case.case
        );
        assert_eq!(
            got["MonitorArn"], json!(arn),
            "case {}: unexpected MonitorArn in {got}",
            case.case
        );
        assert_eq!(
            resource_set(&got, "Resources"),
            case.resources,
            "case {}: unexpected Resources in {got}",
            case.case
        );
        assert_eq!(
            got["Status"], json!(MONITOR_STATE_PENDING),
            "case {}: a just-created monitor must report PENDING, got {got}",
            case.case
        );
        assert_eq!(
            got["ProcessingStatus"], json!("COLLECTING_DATA"),
            "case {}: unexpected ProcessingStatus in {got}",
            case.case
        );

        let expected_tags = case.body.get("Tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            got["Tags"], expected_tags,
            "case {}: unexpected Tags in {got}",
            case.case
        );

        for name in VERBATIM_MEMBERS {
            assert_eq!(
                member(&got, name),
                member(&case.body, name),
                "case {}: member {name} did not round-trip; got {got}",
                case.case
            );
        }

        if let Some(config) = member(&case.body, "HealthEventsConfig") {
            for threshold in ["AvailabilityScoreThreshold", "PerformanceScoreThreshold"] {
                assert_eq!(
                    got["HealthEventsConfig"].get(threshold),
                    config.get(threshold),
                    "case {}: HealthEventsConfig.{threshold} did not round-trip; got {got}",
                    case.case
                );
            }
        }

        // A fresh monitor has not been modified since it was created.
        let created_at = timestamp_secs(&got, "CreatedAt");
        let modified_at = timestamp_secs(&got, "ModifiedAt");
        assert!(
            (modified_at - created_at).abs() < f64::EPSILON,
            "case {}: a fresh monitor must report ModifiedAt == CreatedAt, got {created_at} and {modified_at}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_monitor_arn_carries_the_region_the_request_was_signed_for() {
    let router = emulator();

    // The rule is "the region of the call", so it is checked on more than one region.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let name = format!("monitor-{region}");
        let (status, created) = call_in(
            &router,
            region,
            "POST",
            "/v20210603/Monitors",
            Some(json!({ "MonitorName": name })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: expected 200, got {status} with {created}"
        );
        assert_eq!(
            created["Arn"],
            json!(expected_arn(region, &name)),
            "case {region}: unexpected Arn in {created}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Input constraints: each published bound, and one past it
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn create_monitor_enforces_each_published_input_bound() {
    let router = emulator();

    // (case, body, expected status). The bounds are the model's own: MonitorName
    // len 1..255 pattern ^[a-zA-Z0-9_.-]+$, MaxCityNetworksToMonitor 1..500000,
    // TrafficPercentageToMonitor 1..100
    // (smithy_query(query="operation",operation="CreateMonitor")).
    let too_long = "n".repeat(256);
    let cases: Vec<(&str, Value, u16)> = vec![
        ("name at the minimum length", json!({ "MonitorName": "x" }), 200),
        (
            "name one past the maximum length",
            json!({ "MonitorName": too_long }),
            400,
        ),
        (
            "name outside the published pattern",
            json!({ "MonitorName": "not a valid name!" }),
            400,
        ),
        (
            "city-network maximum at its minimum",
            json!({ "MonitorName": "cn-min", "MaxCityNetworksToMonitor": 1 }),
            200,
        ),
        (
            "city-network maximum at its maximum",
            json!({ "MonitorName": "cn-max", "MaxCityNetworksToMonitor": 500_000 }),
            200,
        ),
        (
            "city-network maximum one below its minimum",
            json!({ "MonitorName": "cn-under", "MaxCityNetworksToMonitor": 0 }),
            400,
        ),
        (
            "city-network maximum one past its maximum",
            json!({ "MonitorName": "cn-over", "MaxCityNetworksToMonitor": 500_001 }),
            400,
        ),
        (
            "traffic percentage at its minimum",
            json!({ "MonitorName": "tp-min", "TrafficPercentageToMonitor": 1 }),
            200,
        ),
        (
            "traffic percentage at its maximum",
            json!({ "MonitorName": "tp-max", "TrafficPercentageToMonitor": 100 }),
            200,
        ),
        (
            "traffic percentage one below its minimum",
            json!({ "MonitorName": "tp-under", "TrafficPercentageToMonitor": 0 }),
            400,
        ),
        (
            "traffic percentage one past its maximum",
            json!({ "MonitorName": "tp-over", "TrafficPercentageToMonitor": 101 }),
            400,
        ),
    ];

    for (case, body, expected) in cases {
        let (status, response) = call(&router, "POST", "/v20210603/Monitors", Some(body)).await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} with {response}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// The clock-owed PENDING -> ACTIVE transition
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_monitor_settles_from_pending_to_active() {
    let router = emulator();

    let (status, created) = call(
        &router,
        "POST",
        "/v20210603/Monitors",
        Some(json!({ "MonitorName": "settling-monitor" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "create: expected 200, got {status} with {created}"
    );
    assert_eq!(
        created["Status"], json!(MONITOR_STATE_PENDING),
        "create: a new monitor must be reported PENDING, got {created}"
    );

    tokio::time::sleep(past_activation()).await;

    let (status, got) = call(&router, "GET", "/v20210603/Monitors/settling-monitor", None).await;
    assert_eq!(
        status, 200,
        "after the activation span: expected 200, got {status} with {got}"
    );
    assert_eq!(
        got["Status"], json!(MONITOR_STATE_ACTIVE),
        "after the activation span the monitor must be ACTIVE, got {got}"
    );

    // Reading does not move it any further: a report is idempotent.
    let (_, again) = call(&router, "GET", "/v20210603/Monitors/settling-monitor", None).await;
    assert_eq!(
        again["Status"], json!(MONITOR_STATE_ACTIVE),
        "a second read must report the same status, got {again}"
    );
    assert_eq!(
        again["ModifiedAt"], got["ModifiedAt"],
        "reading a monitor must not bump ModifiedAt: {got} then {again}"
    );
}

// ---------------------------------------------------------------------------------------
// CreateMonitor conflict and idempotency
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn creating_a_name_that_exists_conflicts_unless_the_client_token_repeats() {
    let router = emulator();

    let first = json!({
        "MonitorName": "conflict-monitor",
        "ClientToken": "token-one",
        "Resources": ["arn:aws:ec2:us-east-1:000000000000:vpc/vpc-dddd4444"],
    });
    let arn = create_monitor(&router, first.clone()).await;

    // Same token: idempotent, and it reports the monitor that token created.
    let (status, repeat) = call(&router, "POST", "/v20210603/Monitors", Some(first)).await;
    assert_eq!(
        status, 200,
        "repeating a ClientToken must be idempotent, got {status} with {repeat}"
    );
    assert_eq!(
        repeat["Arn"], json!(arn),
        "a repeated ClientToken must report the same Arn, got {repeat}"
    );

    // A different token (or none) for a name that exists conflicts.
    for (case, body) in [
        (
            "a different client token",
            json!({ "MonitorName": "conflict-monitor", "ClientToken": "token-two" }),
        ),
        (
            "no client token",
            json!({ "MonitorName": "conflict-monitor" }),
        ),
    ] {
        let (status, response) = call(&router, "POST", "/v20210603/Monitors", Some(body)).await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409 ConflictException, got {status} with {response}"
        );
    }

    // A token recorded for one name is not a repeat of a create of ANOTHER name: that
    // request creates its own monitor rather than reporting the first one's ARN.
    let (status, reused) = call(
        &router,
        "POST",
        "/v20210603/Monitors",
        Some(json!({ "MonitorName": "second-monitor", "ClientToken": "token-one" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "a token reused for a new name must create, got {status} with {reused}"
    );
    assert_eq!(
        reused["Arn"],
        json!(expected_arn(REGION, "second-monitor")),
        "a token reused for a new name must mint that name's ARN, got {reused}"
    );
    let (status, second_get) =
        call(&router, "GET", "/v20210603/Monitors/second-monitor", None).await;
    assert_eq!(
        status, 200,
        "the monitor created under a reused token must be stored, got {status} with {second_get}"
    );

    // And exactly one monitor of that name exists.
    let (_, listed) = call(&router, "GET", "/v20210603/Monitors", None).await;
    let matching = listed["Monitors"]
        .as_array()
        .map(|monitors| {
            monitors
                .iter()
                .filter(|monitor| monitor["MonitorName"] == json!("conflict-monitor"))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(
        matching, 1,
        "a conflicting create must not have stored a second monitor: {listed}"
    );
}

// ---------------------------------------------------------------------------------------
// ListMonitors: every persisted monitor, paging, and the status filter
// ---------------------------------------------------------------------------------------

/// Page through every monitor at `page_size`, returning the names in the order seen.
async fn page_all(router: &MultiServiceRouter, page_size: usize) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than there can be pages, so a token that never clears fails the
    // assertion below instead of looping forever.
    for _ in 0..64 {
        let mut path = format!("/v20210603/Monitors?MaxResults={page_size}");
        if let Some(next) = &token {
            path.push_str(&format!("&NextToken={next}"));
        }
        let (status, page) = call(router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "page size {page_size}: expected 200, got {status} with {page}"
        );
        let monitors = page["Monitors"]
            .as_array()
            .unwrap_or_else(|| panic!("page size {page_size}: no Monitors in {page}"));
        assert!(
            monitors.len() <= page_size,
            "page size {page_size}: a page returned {} monitors",
            monitors.len()
        );
        for monitor in monitors {
            seen.push(monitor["MonitorName"].as_str().unwrap_or_default().to_string());
        }
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
    }
    panic!("page size {page_size}: NextToken never cleared after {} names", seen.len());
}

#[tokio::test]
async fn list_monitors_pages_over_every_persisted_monitor() {
    let router = emulator();

    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        let name = format!("paged-monitor-{index:02}");
        create_monitor(&router, json!({ "MonitorName": name })).await;
        expected.push(name);
    }
    expected.sort();

    // The rule generalises over the page size, so it is checked at the published bounds of
    // MaxResults (1 and 25), at sizes that divide the set and at ones that do not, and with
    // MaxResults absent.
    for page_size in [1_usize, 2, 3, 6, 7, 25] {
        let seen = page_all(&router, page_size).await;
        assert_eq!(
            seen, expected,
            "page size {page_size}: paging must yield every monitor exactly once, got {seen:?}"
        );
    }

    let (status, all) = call(&router, "GET", "/v20210603/Monitors", None).await;
    assert_eq!(status, 200, "no MaxResults: expected 200, got {all}");
    let names: Vec<String> = all["Monitors"]
        .as_array()
        .unwrap_or_else(|| panic!("no MaxResults: no Monitors in {all}"))
        .iter()
        .map(|monitor| monitor["MonitorName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names, expected,
        "no MaxResults must list every monitor in one page, got {names:?}"
    );
    assert!(
        all.get("NextToken").is_none(),
        "a complete page must carry no NextToken: {all}"
    );
}

#[tokio::test]
async fn list_monitors_refuses_a_page_request_it_cannot_serve() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": "token-monitor" })).await;

    // (case, query, expected status). MaxResults is modelled 1..25.
    let cases: Vec<(&str, &str, u16)> = vec![
        ("MaxResults at its minimum", "?MaxResults=1", 200),
        ("MaxResults at its maximum", "?MaxResults=25", 200),
        ("MaxResults one below its minimum", "?MaxResults=0", 400),
        ("MaxResults one past its maximum", "?MaxResults=26", 400),
        ("a NextToken this service never issued", "?NextToken=zzzz", 400),
        ("an unmodelled MonitorStatus", "?MonitorStatus=STOPPED", 400),
    ];

    for (case, query, expected) in cases {
        let (status, response) =
            call(&router, "GET", &format!("/v20210603/Monitors{query}"), None).await;
        assert_eq!(
            status, expected,
            "case {case}: expected {expected}, got {status} with {response}"
        );
    }
}

#[tokio::test]
async fn list_monitors_filters_by_monitor_status() {
    let router = emulator();

    for name in ["filter-active-1", "filter-active-2", "filter-inactive"] {
        create_monitor(&router, json!({ "MonitorName": name })).await;
    }
    // One monitor is taken out of service; the other two are left to settle to ACTIVE.
    let (status, updated) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/filter-inactive",
        Some(json!({ "Status": MONITOR_STATE_INACTIVE })),
    )
    .await;
    assert_eq!(
        status, 200,
        "making a monitor INACTIVE: expected 200, got {status} with {updated}"
    );
    tokio::time::sleep(past_activation()).await;

    // The whole catalog of MonitorConfigState, from the model
    // (smithy_query(query="shape",shape="MonitorConfigState")), each with the names it
    // must select.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("ACTIVE", vec!["filter-active-1", "filter-active-2"]),
        ("INACTIVE", vec!["filter-inactive"]),
        ("PENDING", vec![]),
        ("ERROR", vec![]),
    ];

    for (filter, expected) in cases {
        let (status, page) = call(
            &router,
            "GET",
            &format!("/v20210603/Monitors?MonitorStatus={filter}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {filter}: expected 200, got {status} with {page}"
        );
        let names: Vec<String> = page["Monitors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {filter}: no Monitors in {page}"))
            .iter()
            .map(|monitor| monitor["MonitorName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, expected,
            "case {filter}: unexpected selection, got {names:?}"
        );
        for monitor in page["Monitors"].as_array().unwrap_or(&Vec::new()) {
            assert_eq!(
                monitor["Status"], json!(filter),
                "case {filter}: a selected monitor reports another status: {monitor}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// UpdateMonitor
// ---------------------------------------------------------------------------------------

/// One `Resources` mutation: what the monitor starts with, what the update adds and
/// removes, and the set it must end up with.
struct ResourceCase {
    case: &'static str,
    initial: &'static [&'static str],
    add: &'static [&'static str],
    remove: &'static [&'static str],
    expected: &'static [&'static str],
}

const VPC_A: &str = "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-aaaa0001";
const VPC_B: &str = "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-bbbb0002";
const VPC_C: &str = "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-cccc0003";

const RESOURCE_CASES: &[ResourceCase] = &[
    ResourceCase {
        case: "add two to an empty set",
        initial: &[],
        add: &[VPC_A, VPC_B],
        remove: &[],
        expected: &[VPC_A, VPC_B],
    },
    ResourceCase {
        case: "adding what is already monitored is a union, so nothing changes",
        initial: &[VPC_A],
        add: &[VPC_A],
        remove: &[],
        expected: &[VPC_A],
    },
    ResourceCase {
        case: "removing what is not monitored is a difference, so nothing changes",
        initial: &[VPC_A],
        add: &[],
        remove: &[VPC_C],
        expected: &[VPC_A],
    },
    ResourceCase {
        case: "add and remove in one call",
        initial: &[VPC_A, VPC_B],
        add: &[VPC_C],
        remove: &[VPC_A],
        expected: &[VPC_B, VPC_C],
    },
    ResourceCase {
        case: "remove everything",
        initial: &[VPC_A, VPC_B],
        add: &[],
        remove: &[VPC_A, VPC_B],
        expected: &[],
    },
    ResourceCase {
        case: "a duplicate in the same add collapses",
        initial: &[],
        add: &[VPC_A, VPC_A],
        remove: &[],
        expected: &[VPC_A],
    },
];

#[tokio::test]
async fn update_monitor_applies_resources_as_a_set_union_and_difference() {
    let router = emulator();

    for (index, case) in RESOURCE_CASES.iter().enumerate() {
        let name = format!("resource-case-{index}");
        create_monitor(
            &router,
            json!({ "MonitorName": name, "Resources": case.initial }),
        )
        .await;

        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/v20210603/Monitors/{name}"),
            Some(json!({ "ResourcesToAdd": case.add, "ResourcesToRemove": case.remove })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {updated}",
            case.case
        );
        assert_eq!(
            updated["MonitorArn"],
            json!(expected_arn(REGION, &name)),
            "case {}: unexpected MonitorArn in {updated}",
            case.case
        );

        let (_, got) = call(&router, "GET", &format!("/v20210603/Monitors/{name}"), None).await;
        let expected: BTreeSet<String> = case.expected.iter().map(|arn| (*arn).to_string()).collect();
        assert_eq!(
            resource_set(&got, "Resources"),
            expected,
            "case {}: unexpected Resources in {got}",
            case.case
        );
    }
}

#[tokio::test]
async fn update_monitor_overwrites_only_the_members_it_names_and_bumps_modified_at() {
    let router = emulator();

    create_monitor(
        &router,
        json!({
            "MonitorName": "patched-monitor",
            "MaxCityNetworksToMonitor": 100,
            "TrafficPercentageToMonitor": 10,
            "HealthEventsConfig": { "AvailabilityScoreThreshold": 95.0 },
        }),
    )
    .await;
    let (_, before) = call(&router, "GET", "/v20210603/Monitors/patched-monitor", None).await;

    tokio::time::sleep(Duration::from_millis(20)).await;
    let (status, updated) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/patched-monitor",
        Some(json!({
            "TrafficPercentageToMonitor": 50,
            "InternetMeasurementsLogDelivery": {
                "S3Config": { "BucketName": "patched-bucket", "LogDeliveryStatus": "ENABLED" }
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "update: expected 200, got {status} with {updated}"
    );

    let (_, after) = call(&router, "GET", "/v20210603/Monitors/patched-monitor", None).await;
    assert_eq!(
        after["TrafficPercentageToMonitor"], json!(50),
        "a named member must be overwritten, got {after}"
    );
    assert_eq!(
        after["MaxCityNetworksToMonitor"], json!(100),
        "a member the update did not name must keep its stored value, got {after}"
    );
    assert_eq!(
        after["HealthEventsConfig"]["AvailabilityScoreThreshold"], json!(95.0),
        "a structure the update did not name must keep its stored value, got {after}"
    );
    assert_eq!(
        after["InternetMeasurementsLogDelivery"]["S3Config"]["BucketName"],
        json!("patched-bucket"),
        "a named structure must be overwritten, got {after}"
    );
    assert!(
        timestamp_secs(&after, "ModifiedAt") > timestamp_secs(&before, "ModifiedAt"),
        "an update must bump ModifiedAt: {before} then {after}"
    );
    assert_eq!(
        after["CreatedAt"], before["CreatedAt"],
        "an update must not move CreatedAt: {before} then {after}"
    );
}

#[tokio::test]
async fn update_monitor_accepts_only_the_statuses_its_model_documents() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": "status-monitor" })).await;

    // The model's own doc for UpdateMonitorInput$Status: ACTIVE and INACTIVE are accepted,
    // PENDING and ERROR are not
    // (smithy_query(query="doc",shape="UpdateMonitorInput",member="Status")). A value
    // outside MonitorConfigState is refused by the boundary validator.
    let cases: Vec<(&str, u16)> = vec![
        (MONITOR_STATE_ACTIVE, 200),
        (MONITOR_STATE_INACTIVE, 200),
        (MONITOR_STATE_PENDING, 400),
        ("ERROR", 400),
        ("NOT_A_STATE", 400),
    ];

    for (status_value, expected) in cases {
        let (status, response) = call(
            &router,
            "PATCH",
            "/v20210603/Monitors/status-monitor",
            Some(json!({ "Status": status_value })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {status_value}: expected {expected}, got {status} with {response}"
        );
        if expected == 200 {
            assert_eq!(
                response["Status"], json!(status_value),
                "case {status_value}: the update must report the status it set, got {response}"
            );
            let (_, got) = call(&router, "GET", "/v20210603/Monitors/status-monitor", None).await;
            assert_eq!(
                got["Status"], json!(status_value),
                "case {status_value}: GetMonitor must observe the new status, got {got}"
            );
        }
    }

    // An INACTIVE monitor is not processing data.
    let (_, got) = call(&router, "GET", "/v20210603/Monitors/status-monitor", None).await;
    assert_eq!(
        got["Status"], json!(MONITOR_STATE_INACTIVE),
        "the last accepted status must still be in force, got {got}"
    );
    assert_eq!(
        got["ProcessingStatus"], json!("INACTIVE"),
        "an INACTIVE monitor must report ProcessingStatus INACTIVE, got {got}"
    );
}

#[tokio::test]
async fn update_monitor_is_idempotent_per_client_token() {
    let router = emulator();
    create_monitor(
        &router,
        json!({ "MonitorName": "idempotent-monitor", "Resources": [VPC_A] }),
    )
    .await;

    let body = json!({ "ClientToken": "update-token", "ResourcesToRemove": [VPC_A] });
    let (status, first) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/idempotent-monitor",
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "first update: expected 200, got {status} with {first}"
    );
    let (_, after_first) = call(
        &router,
        "GET",
        "/v20210603/Monitors/idempotent-monitor",
        None,
    )
    .await;

    tokio::time::sleep(Duration::from_millis(20)).await;
    let (status, second) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/idempotent-monitor",
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "repeat update: expected 200, got {status} with {second}"
    );
    let (_, after_second) = call(
        &router,
        "GET",
        "/v20210603/Monitors/idempotent-monitor",
        None,
    )
    .await;
    assert_eq!(
        after_second["ModifiedAt"], after_first["ModifiedAt"],
        "repeating a ClientToken must not apply the update twice: {after_first} then {after_second}"
    );

    // A token is scoped to the request it recorded: reused on ANOTHER monitor it is a
    // different request, so that update is applied rather than answered with the first
    // monitor's result.
    let other_arn = create_monitor(
        &router,
        json!({ "MonitorName": "other-monitor", "Resources": [VPC_A] }),
    )
    .await;
    let (status, crossed) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/other-monitor",
        Some(json!({ "ClientToken": "update-token", "ResourcesToRemove": [VPC_A] })),
    )
    .await;
    assert_eq!(
        status, 200,
        "cross-monitor token reuse: expected 200, got {status} with {crossed}"
    );
    assert_eq!(
        crossed["MonitorArn"],
        json!(other_arn),
        "cross-monitor token reuse must report the monitor the request names, got {crossed}"
    );
    let (_, other_after) = call(&router, "GET", "/v20210603/Monitors/other-monitor", None).await;
    assert_eq!(
        resource_set(&other_after, "Resources"),
        BTreeSet::new(),
        "cross-monitor token reuse must apply the update it names, got {other_after}"
    );
}

// ---------------------------------------------------------------------------------------
// DeleteMonitor
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_monitor_makes_it_unobservable_and_frees_its_name() {
    let router = emulator();
    let arn = create_monitor(
        &router,
        json!({ "MonitorName": "doomed-monitor", "Tags": { "Env": "dev" } }),
    )
    .await;

    let (status, deleted) = call(&router, "DELETE", "/v20210603/Monitors/doomed-monitor", None).await;
    assert_eq!(
        status, 200,
        "delete: expected 200, got {status} with {deleted}"
    );

    let (status, got) = call(&router, "GET", "/v20210603/Monitors/doomed-monitor", None).await;
    assert_eq!(
        status, 404,
        "GetMonitor after a delete must answer not found, got {status} with {got}"
    );

    let (_, listed) = call(&router, "GET", "/v20210603/Monitors", None).await;
    assert_eq!(
        listed["Monitors"], json!([]),
        "ListMonitors after a delete must not report the monitor: {listed}"
    );

    // Its tags went with it.
    let (status, tags) = call(&router, "GET", &format!("/tags/{}", label(&arn)), None).await;
    assert_eq!(
        status, 404,
        "ListTagsForResource for a deleted monitor must be 404, got {status} with {tags}"
    );

    // DeleteMonitor is @idempotent and models no not-found error, so deleting again
    // succeeds.
    let (status, again) = call(&router, "DELETE", "/v20210603/Monitors/doomed-monitor", None).await;
    assert_eq!(
        status, 200,
        "a repeated delete must succeed, got {status} with {again}"
    );

    // And the name is free again.
    let reborn = create_monitor(&router, json!({ "MonitorName": "doomed-monitor" })).await;
    assert_eq!(
        reborn, arn,
        "re-creating a deleted name must mint the same ARN, got {reborn}"
    );
}

// ---------------------------------------------------------------------------------------
// Tagging
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn tag_resource_merges_into_the_map_get_monitor_reports() {
    let router = emulator();
    let arn = create_monitor(
        &router,
        json!({ "MonitorName": "tagged-monitor", "Tags": { "Env": "dev" } }),
    )
    .await;
    let tags_path = format!("/tags/{}", label(&arn));

    // Tagging is not a configuration change, so `ModifiedAt` must not move: the note in
    // `src/ops/tag_resource.rs` says why, and this is what pins it.
    let (_, before) = call(&router, "GET", "/v20210603/Monitors/tagged-monitor", None).await;
    let modified_at = before["ModifiedAt"].clone();

    // (case, tags to merge, the whole map that must result). A merge keeps every key the
    // request does not name and overwrites the ones it does.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the create-time tags are already there",
            json!({}),
            json!({ "Env": "dev" }),
        ),
        (
            "a new key is added",
            json!({ "Team": "net" }),
            json!({ "Env": "dev", "Team": "net" }),
        ),
        (
            "an existing key is overwritten",
            json!({ "Env": "prod" }),
            json!({ "Env": "prod", "Team": "net" }),
        ),
        (
            "several keys at once",
            json!({ "Owner": "sre", "Tier": "1" }),
            json!({ "Env": "prod", "Owner": "sre", "Team": "net", "Tier": "1" }),
        ),
    ];

    for (case, merge, expected) in cases {
        if merge != json!({}) {
            let (status, response) =
                call(&router, "POST", &tags_path, Some(json!({ "Tags": merge }))).await;
            assert_eq!(
                status, 204,
                "case {case}: TagResource expected 204, got {status} with {response}"
            );
        }

        let (status, listed) = call(&router, "GET", &tags_path, None).await;
        assert_eq!(
            status, 200,
            "case {case}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed["Tags"], expected,
            "case {case}: unexpected tag map, got {listed}"
        );

        // The same map, through the monitor itself.
        let (_, got) = call(&router, "GET", "/v20210603/Monitors/tagged-monitor", None).await;
        assert_eq!(
            got["Tags"], expected,
            "case {case}: GetMonitor must report the same tag map, got {got}"
        );
        assert_eq!(
            got["ModifiedAt"], modified_at,
            "case {case}: tagging must not bump ModifiedAt, got {got}"
        );
    }
}

#[tokio::test]
async fn the_tagging_operations_refuse_an_arn_no_monitor_carries() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": "real-monitor" })).await;

    // Each row is an ARN that satisfies the modelled constraints (len 20..512, ^arn:.*)
    // but names no monitor in this emulator.
    let cases: Vec<(&str, String)> = vec![
        (
            "a monitor name that does not exist",
            expected_arn(REGION, "no-such-monitor"),
        ),
        (
            "the right name in another region",
            expected_arn("eu-west-1", "real-monitor"),
        ),
        (
            "another service's resource",
            "arn:aws:ec2:us-east-1:000000000000:vpc/vpc-aaaa0001".to_string(),
        ),
    ];

    for (case, arn) in cases {
        let path = format!("/tags/{}", label(&arn));
        let (status, listed) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case}: ListTagsForResource expected 404, got {status} with {listed}"
        );
        let (status, tagged) =
            call(&router, "POST", &path, Some(json!({ "Tags": { "k": "v" } }))).await;
        assert_eq!(
            status, 404,
            "case {case}: TagResource expected 404, got {status} with {tagged}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Unknown monitors, and the operations this slice deliberately leaves unimplemented
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_monitor_name_no_monitor_carries_is_refused_with_the_modelled_error() {
    let router = emulator();

    // A monitor this account does not hold is a not-found condition for both the read and
    // the mutate: 404 ResourceNotFoundException. The shapes and the shared message are
    // asserted row by row in `crate::refusal_shapes`.
    let (status, got) = call(&router, "GET", "/v20210603/Monitors/ghost-monitor", None).await;
    assert_eq!(
        status, 404,
        "GetMonitor for an unknown name: expected 404, got {status} with {got}"
    );

    let (status, updated) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/ghost-monitor",
        Some(json!({ "TrafficPercentageToMonitor": 5 })),
    )
    .await;
    assert_eq!(
        status, 404,
        "UpdateMonitor for an unknown name: expected 404, got {status} with {updated}"
    );

    // A LinkedAccountId naming an account this emulator does not serve resolves no
    // monitor either.
    create_monitor(&router, json!({ "MonitorName": "linked-monitor" })).await;
    let (status, linked) = call(
        &router,
        "GET",
        "/v20210603/Monitors/linked-monitor?LinkedAccountId=123456789012",
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown LinkedAccountId: expected 400, got {status} with {linked}"
    );
    let (status, own) = call(
        &router,
        "GET",
        &format!("/v20210603/Monitors/linked-monitor?LinkedAccountId={ACCOUNT}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "this emulator's own account id: expected 200, got {status} with {own}"
    );
}

#[tokio::test]
async fn the_reads_of_the_query_and_internet_event_surface_answer_from_state() {
    let router = emulator();
    create_monitor(&router, json!({ "MonitorName": "untouched-monitor" })).await;

    // Both of these were the crate's last scaffolds. Neither answers 501 any more: each
    // resolves what the request names against the store and refuses what it cannot
    // resolve with the modelled 400 `ValidationException`. What each one SERVES is
    // covered end to end by `crate::feed_and_query_results`.
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "GetQueryResults",
            "GET",
            "/v20210603/Monitors/untouched-monitor/Queries/some-query-id/Results".to_string(),
            None,
        ),
        (
            "GetInternetEvent",
            "GET",
            "/v20210603/InternetEvents/no-such-event".to_string(),
            None,
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {response}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// The store's own bounds
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn create_monitor_refuses_to_grow_the_store_past_its_bound() {
    let router = emulator();

    for index in 0..MAX_MONITORS {
        create_monitor(&router, json!({ "MonitorName": format!("bound-{index:04}") })).await;
    }

    // One past the ceiling is the modelled quota error, not an unbounded insert.
    let (status, refused) = call(
        &router,
        "POST",
        "/v20210603/Monitors",
        Some(json!({ "MonitorName": "one-too-many" })),
    )
    .await;
    assert_eq!(
        status, 403,
        "at the monitor ceiling: expected 403 LimitExceededException, got {status} with {refused}"
    );

    // And the refusal stored nothing: freeing one slot makes exactly one create succeed.
    let (status, deleted) = call(&router, "DELETE", "/v20210603/Monitors/bound-0000", None).await;
    assert_eq!(
        status, 200,
        "freeing a slot: expected 200, got {status} with {deleted}"
    );
    create_monitor(&router, json!({ "MonitorName": "one-too-many" })).await;
    let (status, refused_again) = call(
        &router,
        "POST",
        "/v20210603/Monitors",
        Some(json!({ "MonitorName": "and-another" })),
    )
    .await;
    assert_eq!(
        status, 403,
        "back at the ceiling: expected 403, got {status} with {refused_again}"
    );
}

#[tokio::test]
async fn the_idempotency_store_is_bounded_by_count_and_by_retention() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = service_state(&*engine).expect("the service's own store");

    // A caller can send a fresh ClientToken on every UpdateMonitor call, so the record
    // count is capped and the oldest records are the ones evicted.
    let base = std::time::SystemTime::now();
    let overflow = 200_usize;
    {
        let mut store = state.lock();
        for index in 0..(MAX_CLIENT_TOKENS + overflow) {
            store.record_update_token(
                format!("token-{index:06}"),
                format!("monitor-{index}"),
                base + Duration::from_millis(index as u64),
            );
        }
        assert_eq!(
            store.update_client_tokens.len(),
            MAX_CLIENT_TOKENS,
            "the idempotency store must stay at its cap, got {}",
            store.update_client_tokens.len()
        );
        assert!(
            store.update_token_target("token-000000").is_none(),
            "the oldest record must have been evicted"
        );
        let newest = format!("token-{:06}", MAX_CLIENT_TOKENS + overflow - 1);
        assert_eq!(
            store.update_token_target(&newest),
            Some(format!("monitor-{}", MAX_CLIENT_TOKENS + overflow - 1)),
            "the newest record must have been kept"
        );
    }

    // And a record the retention window has already ended is expired by the clock.
    {
        let mut store = state.lock();
        let now = std::time::SystemTime::now();
        let long_ago = now - CLIENT_TOKEN_RETENTION - Duration::from_secs(60);
        store.record_create_token("stale".to_string(), "m-old".to_string(), long_ago);
        assert_eq!(
            store.create_token_target("stale"),
            Some("m-old".to_string()),
            "a record is honoured inside its retention window"
        );
        store.record_create_token("fresh".to_string(), "m-new".to_string(), now);
        assert!(
            store.create_token_target("stale").is_none(),
            "a record past its retention window must be expired, got {:?}",
            store.create_token_target("stale")
        );
        assert_eq!(
            store.create_token_target("fresh"),
            Some("m-new".to_string()),
            "a record inside its retention window must be kept"
        );
    }
}

/// `count` distinct resource ARNs, for the per-monitor ceiling cases.
fn resource_arns(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("arn:aws:ec2:us-east-1:000000000000:vpc/vpc-{index:08}"))
        .collect()
}

/// `count` distinct tags, for the per-monitor ceiling cases.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key-{index:04}"), json!(format!("value-{index}"))))
            .collect(),
    )
}

#[tokio::test]
async fn a_monitors_resource_set_is_bounded() {
    let router = emulator();

    // At the ceiling a create is accepted; one past it is the modelled quota error.
    create_monitor(
        &router,
        json!({ "MonitorName": "full-monitor", "Resources": resource_arns(MAX_RESOURCES_PER_MONITOR) }),
    )
    .await;
    let (status, refused) = call(
        &router,
        "POST",
        "/v20210603/Monitors",
        Some(json!({
            "MonitorName": "over-monitor",
            "Resources": resource_arns(MAX_RESOURCES_PER_MONITOR + 1),
        })),
    )
    .await;
    assert_eq!(
        status, 403,
        "one resource past the ceiling: expected 403 LimitExceededException, got {status} with {refused}"
    );

    // An update that would carry the set past the ceiling is refused, and refused whole:
    // nothing it asked for is applied.
    let (status, over) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/full-monitor",
        Some(json!({ "ResourcesToAdd": ["arn:aws:ec2:us-east-1:000000000000:vpc/vpc-99999999"] })),
    )
    .await;
    assert_eq!(
        status, 403,
        "an update past the ceiling: expected 403, got {status} with {over}"
    );
    let (_, got) = call(&router, "GET", "/v20210603/Monitors/full-monitor", None).await;
    assert_eq!(
        resource_set(&got, "Resources").len(),
        MAX_RESOURCES_PER_MONITOR,
        "a refused update must leave the stored set as it was, got {got}"
    );

    // An update that swaps one for another stays at the ceiling and is accepted, so the
    // bound refuses growth rather than every update.
    let (status, swapped) = call(
        &router,
        "PATCH",
        "/v20210603/Monitors/full-monitor",
        Some(json!({
            "ResourcesToRemove": ["arn:aws:ec2:us-east-1:000000000000:vpc/vpc-00000000"],
            "ResourcesToAdd": ["arn:aws:ec2:us-east-1:000000000000:vpc/vpc-99999999"],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "a swap at the ceiling: expected 200, got {status} with {swapped}"
    );
    let (_, after) = call(&router, "GET", "/v20210603/Monitors/full-monitor", None).await;
    let resources = resource_set(&after, "Resources");
    assert_eq!(
        resources.len(),
        MAX_RESOURCES_PER_MONITOR,
        "a swap must keep the set at its size, got {after}"
    );
    assert!(
        resources.contains("arn:aws:ec2:us-east-1:000000000000:vpc/vpc-99999999")
            && !resources.contains("arn:aws:ec2:us-east-1:000000000000:vpc/vpc-00000000"),
        "a swap must apply both halves, got {after}"
    );
}

#[tokio::test]
async fn a_monitors_tag_map_is_bounded() {
    let router = emulator();

    // The model states the maximum: 50 tags per monitor. At it, a create is accepted.
    let arn = create_monitor(
        &router,
        json!({ "MonitorName": "tag-full-monitor", "Tags": tag_map(MAX_TAGS_PER_MONITOR) }),
    )
    .await;
    let tags_path = format!("/tags/{}", label(&arn));

    // One past it, in a single create, is refused — `TagMap`'s own 0..200 length would
    // have let it through.
    let (status, refused) = call(
        &router,
        "POST",
        "/v20210603/Monitors",
        Some(json!({
            "MonitorName": "tag-over-monitor",
            "Tags": tag_map(MAX_TAGS_PER_MONITOR + 1),
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "one tag past the ceiling at create: expected 400, got {status} with {refused}"
    );

    // A merge that would grow the stored map past the ceiling is refused whole.
    let (status, over) = call(
        &router,
        "POST",
        &tags_path,
        Some(json!({ "Tags": { "one-too-many": "v" } })),
    )
    .await;
    assert_eq!(
        status, 400,
        "a merge past the ceiling: expected 400, got {status} with {over}"
    );
    let (_, listed) = call(&router, "GET", &tags_path, None).await;
    assert_eq!(
        listed["Tags"],
        tag_map(MAX_TAGS_PER_MONITOR),
        "a refused merge must leave the stored map as it was, got {listed}"
    );

    // A merge that only overwrites existing keys does not grow the map, so it is accepted.
    let (status, overwritten) = call(
        &router,
        "POST",
        &tags_path,
        Some(json!({ "Tags": { "key-0000": "rewritten" } })),
    )
    .await;
    assert_eq!(
        status, 204,
        "a merge at the ceiling that overwrites: expected 204, got {status} with {overwritten}"
    );
    let (_, after) = call(&router, "GET", &tags_path, None).await;
    assert_eq!(
        after["Tags"]["key-0000"], json!("rewritten"),
        "an overwrite at the ceiling must be applied, got {after}"
    );
    assert_eq!(
        after["Tags"].as_object().map(serde_json::Map::len),
        Some(MAX_TAGS_PER_MONITOR),
        "an overwrite must keep the map at its size, got {after}"
    );
}
