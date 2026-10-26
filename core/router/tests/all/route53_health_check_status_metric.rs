// SPDX-License-Identifier: CC0-1.0

//! A Route 53 health check publishes its aggregate status as CloudWatch metric data — in
//! CloudWatch.
//!
//! The rule under test: **a health check Route 53 holds is EVALUATED, and each assessment is
//! published as one datapoint of the `AWS/Route53` metric `HealthCheckStatus` dimensioned by
//! that check's own `HealthCheckId` — `1` while the check is considered healthy and `0` while
//! it is considered unhealthy — where the assessment is the one its `Type` documents (an
//! endpoint probe, the state of the CloudWatch alarm it names, the number of its healthy
//! children), the status moves only after `FailureThreshold` consecutive observations disagree
//! with it, `Disabled` makes it always healthy and `Inverted` flips what is reported.** Two
//! crates have to agree for that to hold (Route 53 assesses, CloudWatch answers for the
//! series), so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: the health-check type (every
//! endpoint type, calculated, alarm-backed and routing-control), the `FailureThreshold` at
//! both published bounds and in between, both published `RequestInterval` values, the two
//! members that override the assessment, the state of the alarm an alarm-backed check reads,
//! the `InsufficientDataHealthStatus` it falls back on, and how many of a calculated check's
//! children are healthy. The recorded configuration an issue happens to mention is a single
//! row.
//!
//! Every assertion is made on the series as a CLIENT reads it — through
//! `cloudwatch:GetMetricStatistics` over the health check's dimension — and each row's period
//! is the check's own `RequestInterval`, so one window holds exactly one probe and the
//! assertion is about the sequence of assessments rather than about an aggregate of them.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The endpoint host a Route 53 request addresses, which is also the routing signal the
/// router resolves that service from.
const ROUTE53_HOST: &str = "route53.amazonaws.com";

/// CloudWatch's `X-Amz-Target` prefix and signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The series a health check's assessment is published as.
const NAMESPACE: &str = "AWS/Route53";
const METRIC: &str = "HealthCheckStatus";
const DIMENSION: &str = "HealthCheckId";

/// The two values that series carries.
const HEALTHY: f64 = 1.0;
const UNHEALTHY: f64 = 0.0;

/// How long a row waits for the probes it asserts over, and how often it looks.
///
/// Generous on purpose: the assessments are published on the emulator's own accelerated
/// schedule, so what is being waited for takes a few seconds at most, and the deadline is
/// only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Epoch seconds, the unit a CloudWatch timestamp is read and written in.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// Call one Route 53 operation over its restXml wire, requiring the status `expected`.
async fn route53(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: &str,
    expected: u16,
) -> String {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::HOST, ROUTE53_HOST)
        .header(http::header::CONTENT_TYPE, "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let read = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected} from Route 53 {method} {uri}, got {status} \
         with body {read}"
    );
    read
}

/// The text of the first `<Id>` element of a Route 53 response.
///
/// Deliberately hand-written rather than pulling in an XML crate: the only documents this
/// reads are the emulator's OWN restXml responses, and a body carrying no `Id` panics with
/// the body, so a surprise is never read as a pass.
fn first_id(body: &str, case: &str) -> String {
    body.split_once("<Id>")
        .and_then(|(_, rest)| rest.split_once("</Id>"))
        .map(|(id, _)| id.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: the reply must carry an Id; body {body}"))
}

/// Create one health check with `config` as the body of its `HealthCheckConfig`, and answer
/// the id Route 53 assigned it.
async fn create_health_check(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    config: &str,
) -> String {
    let body = route53(
        router,
        case,
        "POST",
        "/2013-04-01/healthcheck",
        &format!(
            "<CreateHealthCheckRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <CallerReference>{caller_reference}</CallerReference>\
             <HealthCheckConfig>{config}</HealthCheckConfig>\
             </CreateHealthCheckRequest>"
        ),
        201,
    )
    .await;
    first_id(&body, case)
}

/// Call one CloudWatch operation over its awsJson wire, requiring it to succeed.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{CLOUDWATCH_SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with body \
         {parsed}"
    );
    parsed
}

/// The datapoints CloudWatch reports for one health check's status series, as the
/// `(time stamp, value)` of each `period`-wide window, oldest first.
///
/// `period` is the check's own `RequestInterval`, so one window holds exactly one
/// assessment — which this asserts, through the window's `SampleCount`, rather than assumes.
async fn assessments(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    period: i64,
) -> Vec<(f64, f64)> {
    let now = now_secs();
    let reported = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": [{"Name": DIMENSION, "Value": id}],
            "StartTime": now - 1_800.0,
            "EndTime": now + 60.0,
            "Period": period,
            "Statistics": ["Minimum", "Maximum", "SampleCount"],
        }),
    )
    .await;
    let points = reported["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut assessed: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for point in &points {
        let samples = point["SampleCount"].as_f64().unwrap_or_default();
        assert_eq!(
            samples, 1.0,
            "case {case:?}: a window of the check's own interval holds exactly one \
             assessment, got {point} in {reported}"
        );
        let minimum = point["Minimum"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: every datapoint carries a Minimum, got {reported}")
        });
        let maximum = point["Maximum"].as_f64().unwrap_or_default();
        assert_eq!(
            minimum, maximum,
            "case {case:?}: one assessment's Minimum and Maximum are the same value, got \
             {point}"
        );
        let stamp = point["Timestamp"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: every datapoint carries a Timestamp, got {reported}")
        });
        assessed.push((stamp, minimum));
    }
    assessed.sort_by(|left, right| left.0.total_cmp(&right.0));
    assessed
}

/// The assessments of `id`, once the series holds at least `at_least` of them.
async fn at_least_assessments(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    period: i64,
    at_least: usize,
) -> Vec<f64> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let assessed = assessments(router, case, id, period).await;
        if assessed.len() >= at_least || Instant::now() >= deadline {
            assert!(
                assessed.len() >= at_least,
                "case {case:?}: expected at least {at_least} assessments of the health check's \
                 status, got {assessed:?}"
            );
            return assessed.into_iter().map(|(_, value)| value).collect();
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Create an alarm whose state is decided by its missing-data treatment alone, and wait until
/// CloudWatch reports it in `wanted`.
///
/// "breaching — missing data points are treated as breaching the threshold" and
/// "notBreaching — missing data points are treated as being within the threshold", so an alarm
/// watching a series nobody publishes settles in exactly one state and stays there — which is
/// what makes the row that reads it deterministic.
async fn alarm_in_state(router: &MultiServiceRouter, case: &str, name: &str, wanted: &str) {
    let treatment = match wanted {
        "ALARM" => "breaching",
        "OK" => "notBreaching",
        other => panic!("case {case:?}: no missing-data treatment settles an alarm in {other}"),
    };
    cloudwatch(
        router,
        case,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Route53CheckSuite",
            "MetricName": name,
            "Statistic": "Maximum",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": treatment,
        }),
    )
    .await;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = cloudwatch(
            router,
            case,
            "DescribeAlarms",
            &json!({"AlarmNames": [name]}),
        )
        .await;
        let state = described["MetricAlarms"][0]["StateValue"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if state == wanted || Instant::now() >= deadline {
            assert_eq!(
                state, wanted,
                "case {case:?}: expected the alarm {name} in {wanted}, got {described}"
            );
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// What a row's series must look like.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// Exactly this many leading assessments report healthy, and every one after them
    /// reports unhealthy.
    FlipsToUnhealthy(usize),
    /// Exactly this many leading assessments report unhealthy, and every one after them
    /// reports healthy.
    FlipsToHealthy(usize),
    /// Every assessment reports healthy.
    AlwaysHealthy,
    /// Every assessment reports unhealthy.
    AlwaysUnhealthy,
    /// The check publishes no series at all.
    NoSeries,
}

impl Expected {
    /// How many assessments have to be in the series before it can be judged.
    fn needs(self) -> usize {
        match self {
            // One past the flip, so what is asserted is a flip and not merely a prefix.
            Self::FlipsToUnhealthy(run) | Self::FlipsToHealthy(run) => run + 2,
            Self::AlwaysHealthy | Self::AlwaysUnhealthy => 2,
            Self::NoSeries => 0,
        }
    }

    /// Hold `values` to this shape.
    fn holds(self, case: &str, values: &[f64]) {
        match self {
            Self::FlipsToUnhealthy(run) => Self::flip(case, values, HEALTHY, UNHEALTHY, run),
            Self::FlipsToHealthy(run) => Self::flip(case, values, UNHEALTHY, HEALTHY, run),
            Self::AlwaysHealthy => {
                assert!(
                    !values.is_empty() && values.iter().all(|value| *value == HEALTHY),
                    "case {case:?}: every assessment must report healthy, got {values:?}"
                );
            }
            Self::AlwaysUnhealthy => {
                assert!(
                    !values.is_empty() && values.iter().all(|value| *value == UNHEALTHY),
                    "case {case:?}: every assessment must report unhealthy, got {values:?}"
                );
            }
            Self::NoSeries => assert!(
                values.is_empty(),
                "case {case:?}: a check with no probe schedule publishes no series, got \
                 {values:?}"
            ),
        }
    }

    /// `run` leading assessments of `before`, then nothing but `after`.
    fn flip(case: &str, values: &[f64], before: f64, after: f64, run: usize) {
        let leading = values.iter().take_while(|value| **value == before).count();
        assert_eq!(
            leading, run,
            "case {case:?}: expected {run} assessments of {before} before the status moves, \
             got {values:?}"
        );
        assert!(
            values.len() > leading && values[leading..].iter().all(|value| *value == after),
            "case {case:?}: the status must stay at {after} once it has moved, got {values:?}"
        );
    }
}

/// One row: a health check to create, the period its series is read at, and the shape that
/// series must have.
struct Row {
    case: &'static str,
    /// The body of the `HealthCheckConfig` the row creates.
    config: String,
    /// The check's own `RequestInterval`, which is the period one assessment occupies.
    period: i64,
    expected: Expected,
}

/// The `RequestInterval` a check that names none is given
/// (`HealthCheckConfig$RequestInterval`: "If you don't specify a value for RequestInterval,
/// the default value is 30 seconds"), which is also the period the two types that probe no
/// endpoint publish at.
const DEFAULT_INTERVAL: i64 = 30;

#[tokio::test]
async fn every_form_of_health_check_publishes_the_status_series_its_type_documents() {
    let router = build_router();

    // Two children for the calculated rows, each in a status of its own: an endpoint check
    // that flips to unhealthy on its first failing probe, and a disabled one, which "Route 53
    // considers the status of […] to always be healthy". The unhealthy one is waited for, so
    // every assessment of the parents that read it sees the status it is prepared in.
    let failing_child = create_health_check(
        &router,
        "a child that goes unhealthy",
        "child-unhealthy",
        "<Type>TCP</Type><IPAddress>192.0.2.10</IPAddress><Port>443</Port>\
         <RequestInterval>10</RequestInterval><FailureThreshold>1</FailureThreshold>",
    )
    .await;
    let healthy_child = create_health_check(
        &router,
        "a child that stays healthy",
        "child-healthy",
        "<Type>TCP</Type><IPAddress>192.0.2.11</IPAddress><Port>443</Port>\
         <RequestInterval>10</RequestInterval><FailureThreshold>1</FailureThreshold>\
         <Disabled>true</Disabled>",
    )
    .await;
    let child_values =
        at_least_assessments(&router, "a child that goes unhealthy", &failing_child, 10, 2).await;
    Expected::AlwaysUnhealthy.holds("a child that goes unhealthy", &child_values);

    // Two alarms, each settled in a state of its own, for the alarm-backed rows.
    alarm_in_state(&router, "an alarm in ALARM", "endpoint-breached", "ALARM").await;
    alarm_in_state(&router, "an alarm in OK", "endpoint-within", "OK").await;

    // The rows differ along the type, the threshold, the interval, the two members that
    // override the assessment, the alarm state read, the insufficient-data fallback and the
    // number of healthy children.
    let rows: Vec<Row> = vec![
        Row {
            case: "the recorded configuration: a TCP check at the slow interval",
            config: "<Type>TCP</Type><IPAddress>8.8.8.8</IPAddress><Port>8081</Port>\
                     <RequestInterval>30</RequestInterval><FailureThreshold>5</FailureThreshold>"
                .to_string(),
            period: 30,
            // The fifth consecutive failure is what moves the status, so four assessments
            // report healthy before it.
            expected: Expected::FlipsToUnhealthy(4),
        },
        Row {
            case: "an HTTP check that names neither interval nor threshold",
            config: "<Type>HTTP</Type>\
                     <FullyQualifiedDomainName>probe.example.com</FullyQualifiedDomainName>\
                     <Port>80</Port>"
                .to_string(),
            period: DEFAULT_INTERVAL,
            // The documented default threshold is three health checks.
            expected: Expected::FlipsToUnhealthy(2),
        },
        Row {
            case: "an HTTPS check at the published minimum threshold",
            config: "<Type>HTTPS</Type><IPAddress>192.0.2.20</IPAddress><Port>443</Port>\
                     <RequestInterval>10</RequestInterval><FailureThreshold>1</FailureThreshold>"
                .to_string(),
            period: 10,
            expected: Expected::FlipsToUnhealthy(0),
        },
        Row {
            case: "a TCP check at the published maximum threshold",
            config: "<Type>TCP</Type><IPAddress>192.0.2.21</IPAddress><Port>22</Port>\
                     <RequestInterval>10</RequestInterval><FailureThreshold>10</FailureThreshold>"
                .to_string(),
            period: 10,
            expected: Expected::FlipsToUnhealthy(9),
        },
        Row {
            case: "a string-matching check at the fast interval",
            config: "<Type>HTTP_STR_MATCH</Type><IPAddress>192.0.2.22</IPAddress><Port>80</Port>\
                     <SearchString>SERVING</SearchString>\
                     <RequestInterval>10</RequestInterval><FailureThreshold>2</FailureThreshold>"
                .to_string(),
            period: 10,
            expected: Expected::FlipsToUnhealthy(1),
        },
        Row {
            case: "an inverted endpoint check",
            config: "<Type>HTTPS_STR_MATCH</Type><IPAddress>192.0.2.23</IPAddress>\
                     <Port>443</Port><SearchString>SERVING</SearchString>\
                     <RequestInterval>30</RequestInterval><FailureThreshold>3</FailureThreshold>\
                     <Inverted>true</Inverted>"
                .to_string(),
            period: 30,
            // Inverted reports the opposite of the assessment, so the series starts
            // unhealthy and moves to healthy when the endpoint's status flips.
            expected: Expected::FlipsToHealthy(2),
        },
        Row {
            case: "a disabled endpoint check",
            config: "<Type>TCP</Type><IPAddress>192.0.2.24</IPAddress><Port>443</Port>\
                     <RequestInterval>30</RequestInterval><FailureThreshold>3</FailureThreshold>\
                     <Disabled>true</Disabled>"
                .to_string(),
            period: 30,
            expected: Expected::AlwaysHealthy,
        },
        Row {
            case: "a disabled and inverted endpoint check",
            config: "<Type>TCP</Type><IPAddress>192.0.2.25</IPAddress><Port>443</Port>\
                     <RequestInterval>30</RequestInterval><FailureThreshold>3</FailureThreshold>\
                     <Disabled>true</Disabled><Inverted>true</Inverted>"
                .to_string(),
            period: 30,
            expected: Expected::AlwaysUnhealthy,
        },
        Row {
            case: "a calculated check that needs no healthy child",
            config: "<Type>CALCULATED</Type><HealthThreshold>0</HealthThreshold>".to_string(),
            period: DEFAULT_INTERVAL,
            expected: Expected::AlwaysHealthy,
        },
        Row {
            case: "a calculated check whose only child is unhealthy",
            config: format!(
                "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                 <ChildHealthChecks><ChildHealthCheck>{failing_child}</ChildHealthCheck>\
                 </ChildHealthChecks>"
            ),
            period: DEFAULT_INTERVAL,
            expected: Expected::AlwaysUnhealthy,
        },
        Row {
            case: "a calculated check whose only child is healthy",
            config: format!(
                "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                 <ChildHealthChecks><ChildHealthCheck>{healthy_child}</ChildHealthCheck>\
                 </ChildHealthChecks>"
            ),
            period: DEFAULT_INTERVAL,
            expected: Expected::AlwaysHealthy,
        },
        Row {
            case: "a calculated check that needs more healthy children than it has",
            config: format!(
                "<Type>CALCULATED</Type><HealthThreshold>2</HealthThreshold>\
                 <ChildHealthChecks><ChildHealthCheck>{healthy_child}</ChildHealthCheck>\
                 </ChildHealthChecks>"
            ),
            period: DEFAULT_INTERVAL,
            expected: Expected::AlwaysUnhealthy,
        },
        Row {
            case: "an alarm-backed check whose alarm is in ALARM",
            config: "<Type>CLOUDWATCH_METRIC</Type>\
                     <AlarmIdentifier><Region>us-east-1</Region>\
                     <Name>endpoint-breached</Name></AlarmIdentifier>\
                     <InsufficientDataHealthStatus>Healthy</InsufficientDataHealthStatus>"
                .to_string(),
            period: DEFAULT_INTERVAL,
            // The fallback is Healthy, so an unhealthy assessment can only have come from
            // the alarm's own state.
            expected: Expected::AlwaysUnhealthy,
        },
        Row {
            case: "an alarm-backed check whose alarm is in OK",
            config: "<Type>CLOUDWATCH_METRIC</Type>\
                     <AlarmIdentifier><Region>us-east-1</Region>\
                     <Name>endpoint-within</Name></AlarmIdentifier>\
                     <InsufficientDataHealthStatus>Unhealthy</InsufficientDataHealthStatus>"
                .to_string(),
            period: DEFAULT_INTERVAL,
            // The fallback is Unhealthy, so a healthy assessment can only have come from the
            // alarm's own state.
            expected: Expected::AlwaysHealthy,
        },
        Row {
            case: "an alarm-backed check whose alarm nobody created, falling back to Unhealthy",
            config: "<Type>CLOUDWATCH_METRIC</Type>\
                     <AlarmIdentifier><Region>us-east-1</Region>\
                     <Name>no-such-alarm-unhealthy</Name></AlarmIdentifier>\
                     <InsufficientDataHealthStatus>Unhealthy</InsufficientDataHealthStatus>"
                .to_string(),
            period: DEFAULT_INTERVAL,
            expected: Expected::AlwaysUnhealthy,
        },
        Row {
            case: "an alarm-backed check whose alarm nobody created, falling back to Healthy",
            config: "<Type>CLOUDWATCH_METRIC</Type>\
                     <AlarmIdentifier><Region>us-east-1</Region>\
                     <Name>no-such-alarm-healthy</Name></AlarmIdentifier>\
                     <InsufficientDataHealthStatus>Healthy</InsufficientDataHealthStatus>"
                .to_string(),
            period: DEFAULT_INTERVAL,
            expected: Expected::AlwaysHealthy,
        },
        Row {
            case: "an alarm-backed check falling back to its last known status",
            config: "<Type>CLOUDWATCH_METRIC</Type>\
                     <AlarmIdentifier><Region>us-east-1</Region>\
                     <Name>no-such-alarm-last-known</Name></AlarmIdentifier>\
                     <InsufficientDataHealthStatus>LastKnownStatus\
                     </InsufficientDataHealthStatus>"
                .to_string(),
            period: DEFAULT_INTERVAL,
            // "For new health checks that have no last known status, the default status for
            // the health check is healthy."
            expected: Expected::AlwaysHealthy,
        },
        Row {
            case: "a routing-control follower",
            config: "<Type>RECOVERY_CONTROL</Type>\
                     <RoutingControlArn>arn:aws:route53-recovery-control::111122223333:\
                     controlpanel/0123456789abcdef/routingcontrol/fedcba9876543210\
                     </RoutingControlArn>"
                .to_string(),
            period: DEFAULT_INTERVAL,
            expected: Expected::NoSeries,
        },
    ];

    // Every row's check is created FIRST, so the schedules run side by side instead of one
    // after another.
    let mut ids: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        ids.push(
            create_health_check(&router, row.case, &format!("row-{index}"), &row.config).await,
        );
    }

    for (row, id) in rows.iter().zip(&ids) {
        let values =
            at_least_assessments(&router, row.case, id, row.period, row.expected.needs()).await;
        row.expected.holds(row.case, &values);
    }
}

#[tokio::test]
async fn the_status_series_is_identified_by_the_health_check_it_assesses() {
    let router = build_router();

    // Two checks of DIFFERENT configurations, so the two series are told apart by their
    // values as well as by their dimension.
    let flips = create_health_check(
        &router,
        "a check that flips",
        "identity-flips",
        "<Type>TCP</Type><IPAddress>192.0.2.30</IPAddress><Port>443</Port>\
         <RequestInterval>10</RequestInterval><FailureThreshold>3</FailureThreshold>",
    )
    .await;
    let disabled = create_health_check(
        &router,
        "a check that stays healthy",
        "identity-healthy",
        "<Type>TCP</Type><IPAddress>192.0.2.31</IPAddress><Port>443</Port>\
         <RequestInterval>10</RequestInterval><FailureThreshold>3</FailureThreshold>\
         <Disabled>true</Disabled>",
    )
    .await;
    let flipping = at_least_assessments(&router, "a check that flips", &flips, 10, 4).await;
    Expected::FlipsToUnhealthy(2).holds("a check that flips", &flipping);
    let steady = at_least_assessments(&router, "a check that stays healthy", &disabled, 10, 2).await;
    Expected::AlwaysHealthy.holds("a check that stays healthy", &steady);

    // A metric's identity includes its dimension set, so the series answers only the request
    // that names the health check it is the assessment of.
    let now = now_secs();
    // (case, the dimensions the request names, whether any datapoint answers it)
    let dimension_rows: Vec<(&str, Value, bool)> = vec![
        (
            "the health check's own id",
            json!([{"Name": DIMENSION, "Value": flips}]),
            true,
        ),
        ("no dimension at all", json!([]), false),
        (
            "an id no health check has",
            json!([{"Name": DIMENSION, "Value": "00000000-0000-0000-0000-000000000000"}]),
            false,
        ),
        (
            "the id under another dimension name",
            json!([{"Name": "HealthCheck", "Value": flips}]),
            false,
        ),
    ];
    for (case, dimensions, answered) in dimension_rows {
        let reported = cloudwatch(
            &router,
            case,
            "GetMetricStatistics",
            &json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "StartTime": now - 1_800.0,
                "EndTime": now + 60.0,
                "Period": 60,
                "Statistics": ["Minimum"],
            }),
        )
        .await;
        let points = reported["Datapoints"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            !points.is_empty(),
            answered,
            "case {case:?}: expected datapoints: {answered}, got {reported}"
        );
    }

    // The series live in CloudWatch's own metric store, so CloudWatch LISTS them: one metric
    // per health check, in the namespace and under the dimension Route 53 publishes.
    let listed = cloudwatch(
        &router,
        "the listed series",
        "ListMetrics",
        &json!({"Namespace": NAMESPACE, "MetricName": METRIC}),
    )
    .await;
    let dimensioned: Vec<String> = listed["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|metric| {
            let dimensions = metric["Dimensions"].as_array()?;
            let named = dimensions.iter().find(|dimension| {
                dimension["Name"].as_str() == Some(DIMENSION)
            })?;
            named["Value"].as_str().map(str::to_string)
        })
        .collect();
    for id in [&flips, &disabled] {
        assert!(
            dimensioned.contains(id),
            "case the listed series: expected the series of {id} to be listed, got {listed}"
        );
    }
}
