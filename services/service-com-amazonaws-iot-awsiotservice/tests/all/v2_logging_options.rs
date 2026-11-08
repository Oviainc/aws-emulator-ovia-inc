// SPDX-License-Identifier: CC0-1.0

//! The account's V2 LOGGING OPTIONS, end to end over HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's
//! service is mounted in, so routing, the restJson1 `GET /v2LoggingOptions` and
//! `POST /v2LoggingOptions` bindings, the handlers and the response serialization are
//! all exercised.
//!
//! The rules under test, none of them specific to one set of options:
//!
//! * `GetV2LoggingOptions` reports the options this service HOLDS — what the last
//!   `SetV2LoggingOptions` persisted, member for member;
//! * an account that has never set them holds NO options, so the read is refused with
//!   the modelled `NotConfiguredException` (404) rather than answered with options
//!   nobody configured;
//! * the read is a read: repeating it, with no set between, reports the same options
//!   and leaves the store alone — an unconfigured account stays unconfigured;
//! * a set replaces the account's options in FULL, so a member one request supplies
//!   and the next omits is gone from what the reader reports;
//! * the per-event-type configurations are what `verbose` asks for: the flag "is used
//!   to get all the event types and their respective configuration that event-based
//!   logging supports"
//!   (`smithy_query(query="doc",shape="GetV2LoggingOptionsRequest",member="verbose")`),
//!   and they are reported in `eventType` order whatever order the set listed them in;
//! * a value outside the `LogLevel` catalog the model declares is refused (400) and
//!   changes nothing, and a list past the emulator's own ceiling on the caller-grown
//!   override list is refused with the operation's modelled `InvalidRequestException`;
//! * an event type named twice is configured once, by the LAST configuration given
//!   for it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::LOG_LEVEL_VALUES;
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, LoggedEventConfiguration, V2LoggingOptions,
    MAX_LOG_EVENT_CONFIGURATIONS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. The options are
/// account-wide, so no assertion depends on it.
const REGION: &str = "ap-south-1";

/// A role ARN a caller may name as the one "that allows IoT to write to Cloudwatch
/// logs". Its account segment is the emulator's, and nothing asserts on the text: the
/// member is an opaque `AwsArn` in the model.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/iot-logs";

/// AWS's own sentence for a read of an account whose V2 logging options were never
/// set, written out here so the wording is pinned by the test and not only by the
/// constant under test.
const EXPECTED_NOT_CONFIGURED_MESSAGE: &str =
    "SetV2LoggingOptions was not previously called. No logging options have been set.";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// This service's store, as the handlers reach it.
    fn state(&self) -> engine::ServiceState<ComAmazonawsIotAwsiotserviceState> {
        self.engine.state::<ComAmazonawsIotAwsiotserviceState>()
    }

    /// Overwrite the stored options, so a read can be tested against a record no
    /// request in the test wrote.
    fn seed(&self, options: V2LoggingOptions) {
        self.state().lock().v2_logging_options = Some(options);
    }

    /// The options the store holds, as the handlers see them.
    fn stored(&self) -> Option<V2LoggingOptions> {
        self.state().lock().v2_logging_options.clone()
    }

    async fn dispatch(&self, method: &str, query: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}/v2LoggingOptions{query}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The credential scope names the `iot` signing name the model's
            // `aws.auth#sigv4` trait declares, which is what routes the request.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the v2-logging-options request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    /// `GetV2LoggingOptions`, with `verbose` bound to the query string the model
    /// declares for it when the caller sets it.
    async fn get_v2_logging_options(&self, verbose: bool) -> Reply {
        let query = if verbose { "?verbose=true" } else { "" };
        self.dispatch("GET", query, None).await
    }

    async fn set_v2_logging_options(&self, body: &Value) -> Reply {
        self.dispatch("POST", "", Some(body)).await
    }

    /// Set options the service must accept, failing the case if it does not.
    async fn set_ok(&self, case: &str, body: &Value) {
        let reply = self.set_v2_logging_options(body).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected SetV2LoggingOptions to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    /// The options a successful read reports, as JSON.
    async fn reported(&self, case: &str, verbose: bool) -> Value {
        let reply = self.get_v2_logging_options(verbose).await;
        assert_eq!(
            reply.status,
            200,
            "case {case} (verbose {verbose}): expected GetV2LoggingOptions to answer 200, got {} \
             body {}",
            reply.status,
            reply.text()
        );
        reply.json()
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// The error code a restJson1 error response carries.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }
}

/// One `LogEventConfiguration`, in the JSON form the model binds it to. A member left
/// `None` is left out of the object, which is how a caller omits it.
fn event_configuration(
    event_type: &str,
    log_level: Option<&str>,
    log_destination: Option<&str>,
) -> Value {
    let mut configuration = Map::new();
    configuration.insert(
        "eventType".to_string(),
        Value::String(event_type.to_string()),
    );
    if let Some(log_level) = log_level {
        configuration.insert("logLevel".to_string(), Value::String(log_level.to_string()));
    }
    if let Some(log_destination) = log_destination {
        configuration.insert(
            "logDestination".to_string(),
            Value::String(log_destination.to_string()),
        );
    }
    Value::Object(configuration)
}

/// The event configurations a set supplied, in the `eventType` order a read reports
/// them in.
fn in_event_type_order(supplied: &Value) -> Vec<Value> {
    let mut configurations: Vec<Value> = supplied
        .as_array()
        .cloned()
        .expect("eventConfigurations is a list");
    configurations.sort_by_key(|configuration| {
        configuration["eventType"]
            .as_str()
            .expect("eventType is a string")
            .to_string()
    });
    configurations
}

/// Assert that a read reports exactly what `request` configured.
///
/// Every member is checked by NAME rather than by comparing whole documents, because
/// `disableAllLogs` carries a modelled default and a serializer may leave a default
/// value out — reading `false` for an absent member is the same answer.
async fn assert_reports(emulator: &Emulator, case: &str, request: &Value) {
    for verbose in [false, true] {
        let reported = emulator.reported(case, verbose).await;

        for member in ["roleArn", "defaultLogLevel"] {
            let expected = request.get(member);
            assert_eq!(
                reported.get(member),
                expected,
                "case {case} (verbose {verbose}): expected {member} {expected:?}, got {:?} in {}",
                reported.get(member),
                reported
            );
        }

        let expected_disabled = request
            .get("disableAllLogs")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let reported_disabled = reported
            .get("disableAllLogs")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(
            reported_disabled, expected_disabled,
            "case {case} (verbose {verbose}): expected disableAllLogs {expected_disabled}, got \
             {reported_disabled} in {reported}"
        );

        // The flag is what asks for the per-event-type configurations, so a read that
        // does not set it reports none at all, and a verbose read of an account with
        // no override reports an empty list: the member's presence tracks the flag.
        let expected_events = verbose.then(|| match request.get("eventConfigurations") {
            Some(supplied) => in_event_type_order(supplied),
            None => Vec::new(),
        });
        let reported_events = reported
            .get("eventConfigurations")
            .map(|value| value.as_array().cloned().expect("a list of configurations"));
        assert_eq!(
            reported_events, expected_events,
            "case {case} (verbose {verbose}): expected eventConfigurations {expected_events:?}, \
             got {reported_events:?} in {reported}"
        );
    }
}

/// One set of options that must round-trip through the reader.
struct RoundTripCase {
    name: String,
    request: Value,
}

/// Options spanning what the rule generalises over: which members are supplied, every
/// log level the model declares, both values of `disableAllLogs`, and the number,
/// completeness and order of the per-event-type overrides.
fn round_trip_cases() -> Vec<RoundTripCase> {
    let mut cases = vec![
        RoundTripCase {
            name: "no member at all: configured, and empty".to_string(),
            request: json!({}),
        },
        RoundTripCase {
            name: "a role and a default level".to_string(),
            request: json!({ "roleArn": ROLE_ARN, "defaultLogLevel": "WARN" }),
        },
        RoundTripCase {
            name: "a default level with no role".to_string(),
            request: json!({ "defaultLogLevel": "ERROR" }),
        },
        RoundTripCase {
            name: "all logs disabled".to_string(),
            request: json!({
                "roleArn": ROLE_ARN,
                "defaultLogLevel": "DISABLED",
                "disableAllLogs": true,
            }),
        },
        RoundTripCase {
            name: "logs explicitly not disabled".to_string(),
            request: json!({ "roleArn": ROLE_ARN, "disableAllLogs": false }),
        },
        RoundTripCase {
            name: "one event override, level and destination".to_string(),
            request: json!({
                "roleArn": ROLE_ARN,
                "defaultLogLevel": "INFO",
                "eventConfigurations": [
                    event_configuration("Connect", Some("DEBUG"), Some("iot/connect-logs")),
                ],
            }),
        },
        RoundTripCase {
            name: "one event override naming neither level nor destination".to_string(),
            request: json!({
                "defaultLogLevel": "INFO",
                "eventConfigurations": [event_configuration("Disconnect", None, None)],
            }),
        },
        RoundTripCase {
            name: "three event overrides, supplied out of event-type order".to_string(),
            request: json!({
                "roleArn": ROLE_ARN,
                "defaultLogLevel": "WARN",
                "eventConfigurations": [
                    event_configuration("Subscribe", Some("ERROR"), None),
                    event_configuration("Connect", Some("DEBUG"), Some("iot/connect-logs")),
                    event_configuration("Publish-In", None, Some("iot/publish-logs")),
                ],
            }),
        },
        RoundTripCase {
            name: "an empty list of event overrides".to_string(),
            request: json!({ "defaultLogLevel": "INFO", "eventConfigurations": [] }),
        },
    ];

    // Every value `LogLevel` declares, as the model's own catalog
    // (`smithy_query(query="shape",shape="LogLevel")`), so the rows cover levels the
    // issue never cited — and each one twice over, as the account default and as one
    // event type's override.
    for level in LOG_LEVEL_VALUES {
        cases.push(RoundTripCase {
            name: format!("default log level {level}"),
            request: json!({ "roleArn": ROLE_ARN, "defaultLogLevel": level }),
        });
        cases.push(RoundTripCase {
            name: format!("event override at log level {level}"),
            request: json!({
                "roleArn": ROLE_ARN,
                "eventConfigurations": [event_configuration("Publish-Out", Some(level), None)],
            }),
        });
    }

    cases
}

/// An account that has never set V2 logging options holds none, so the read is refused
/// with the modelled `NotConfiguredException` (404) — for both values of `verbose` —
/// and reading it is a read: the account is still unconfigured afterwards.
#[tokio::test]
async fn an_account_with_no_options_is_refused_as_not_configured() {
    let emulator = Emulator::new();

    for round in 0..3 {
        for verbose in [false, true] {
            let case = format!("round {round}, verbose {verbose}");
            let reply = emulator.get_v2_logging_options(verbose).await;
            assert_eq!(
                reply.status,
                404,
                "case {case}: expected GetV2LoggingOptions to answer 404, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.error_code(),
                "NotConfiguredException",
                "case {case}: expected NotConfiguredException, got {}",
                reply.text()
            );
            assert_eq!(
                reply.json()["message"].as_str(),
                Some(EXPECTED_NOT_CONFIGURED_MESSAGE),
                "case {case}: expected the refusal to name the call that configures the \
                 options, got {}",
                reply.text()
            );
            assert_eq!(
                emulator.stored(),
                None,
                "case {case}: expected the read to leave the account unconfigured, got {:?}",
                emulator.stored()
            );
        }
    }
}

/// Every set of options a caller writes is reported back by the reader, member for
/// member, with the per-event-type overrides answered only to a verbose read.
#[tokio::test]
async fn every_set_of_options_a_caller_writes_is_reported_back() {
    for case in round_trip_cases() {
        // A fresh emulator per row, so each row is read against its own write and no
        // row can pass on another row's state.
        let emulator = Emulator::new();
        emulator.set_ok(&case.name, &case.request).await;
        assert_reports(&emulator, &case.name, &case.request).await;
    }
}

/// Reading configured options is a read: three calls with no set between them report
/// the same options and leave the record alone.
#[tokio::test]
async fn reading_configured_options_changes_nothing() {
    let emulator = Emulator::new();
    let request = json!({
        "roleArn": ROLE_ARN,
        "defaultLogLevel": "INFO",
        "eventConfigurations": [
            event_configuration("Connect", Some("DEBUG"), Some("iot/connect-logs")),
        ],
    });
    emulator.set_ok("repeated read", &request).await;
    let before = emulator.stored();

    for round in 0..3 {
        let case = format!("round {round}");
        assert_reports(&emulator, &case, &request).await;
        assert_eq!(
            emulator.stored(),
            before,
            "case {case}: expected the read to leave the stored options alone, got {:?}",
            emulator.stored()
        );
    }
}

/// A set replaces the account's options in FULL: what the second request omits is gone
/// from what the reader reports.
#[tokio::test]
async fn a_later_set_replaces_the_options_in_full() {
    let sequences: &[(&str, Value, Value)] = &[
        (
            "every member, then only a level",
            json!({
                "roleArn": ROLE_ARN,
                "defaultLogLevel": "DEBUG",
                "disableAllLogs": true,
                "eventConfigurations": [
                    event_configuration("Connect", Some("DEBUG"), Some("iot/connect-logs")),
                ],
            }),
            json!({ "defaultLogLevel": "ERROR" }),
        ),
        (
            "a level, then every member",
            json!({ "defaultLogLevel": "ERROR" }),
            json!({
                "roleArn": ROLE_ARN,
                "defaultLogLevel": "WARN",
                "disableAllLogs": true,
                "eventConfigurations": [
                    event_configuration("Subscribe", Some("INFO"), None),
                    event_configuration("Connect", None, Some("iot/connect-logs")),
                ],
            }),
        ),
        (
            "two event overrides, then one",
            json!({
                "eventConfigurations": [
                    event_configuration("Subscribe", Some("INFO"), None),
                    event_configuration("Connect", Some("DEBUG"), None),
                ],
            }),
            json!({ "eventConfigurations": [event_configuration("Connect", Some("WARN"), None)] }),
        ),
    ];

    for (case, first, second) in sequences {
        let emulator = Emulator::new();
        emulator.set_ok(case, first).await;
        assert_reports(&emulator, &format!("{case}: first set"), first).await;
        emulator.set_ok(case, second).await;
        assert_reports(&emulator, &format!("{case}: second set"), second).await;
    }
}

/// The reader reports what STATE holds, not a value compiled into it: a record seeded
/// straight into the store is what the next read answers with.
#[tokio::test]
async fn the_reader_reports_what_the_store_holds() {
    // Two seeded records, so no single hardcoded response can satisfy both rows.
    let seeds: Vec<(&str, V2LoggingOptions, Value)> = vec![
        (
            "a seeded role and level",
            V2LoggingOptions {
                role_arn: Some(ROLE_ARN.to_string()),
                default_log_level: Some("WARN".to_string()),
                disable_all_logs: false,
                event_configurations: Default::default(),
            },
            json!({ "roleArn": ROLE_ARN, "defaultLogLevel": "WARN" }),
        ),
        (
            "a seeded record with everything disabled and one override",
            V2LoggingOptions {
                role_arn: None,
                default_log_level: Some("DISABLED".to_string()),
                disable_all_logs: true,
                event_configurations: [(
                    "Connect".to_string(),
                    LoggedEventConfiguration {
                        log_level: Some("DEBUG".to_string()),
                        log_destination: Some("iot/connect-logs".to_string()),
                    },
                )]
                .into_iter()
                .collect(),
            },
            json!({
                "defaultLogLevel": "DISABLED",
                "disableAllLogs": true,
                "eventConfigurations": [
                    event_configuration("Connect", Some("DEBUG"), Some("iot/connect-logs")),
                ],
            }),
        ),
    ];

    for (case, seed, expected) in seeds {
        let emulator = Emulator::new();
        emulator.seed(seed);
        assert_reports(&emulator, case, &expected).await;
    }
}

/// A `logLevel` outside the catalog the model declares is refused and changes nothing.
///
/// The value set is a MODELLED constraint, so no request carrying a value outside one
/// ever reaches the handler — which is why the status, and not the error code, is what
/// this asserts.
#[tokio::test]
async fn a_log_level_outside_the_modelled_enum_is_refused_and_changes_nothing() {
    let mut cases: Vec<(String, Value)> = Vec::new();
    for level in LOG_LEVEL_VALUES {
        // The lowercase spelling of a declared value, and a level no catalog lists:
        // both are outside the enum, so both are refused.
        cases.push((
            format!("defaultLogLevel {}", level.to_lowercase()),
            json!({ "defaultLogLevel": level.to_lowercase() }),
        ));
        cases.push((
            format!("event override at {}", level.to_lowercase()),
            json!({
                "eventConfigurations": [
                    event_configuration("Connect", Some(&level.to_lowercase()), None),
                ],
            }),
        ));
    }
    cases.push((
        "a level no catalog lists".to_string(),
        json!({ "defaultLogLevel": "TRACE" }),
    ));

    for (case, request) in cases {
        let emulator = Emulator::new();
        let reply = emulator.set_v2_logging_options(&request).await;
        assert_eq!(
            reply.status,
            400,
            "case {case}: expected SetV2LoggingOptions to refuse with 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            emulator.stored(),
            None,
            "case {case}: expected the refusal to record nothing, got {:?}",
            emulator.stored()
        );
    }
}

/// The emulator's ceiling on the caller-grown override list is a ceiling, not a
/// refusal of every long list: exactly `MAX_LOG_EVENT_CONFIGURATIONS` overrides are
/// recorded and reported, and one more is refused with the operation's modelled
/// `InvalidRequestException` (400) — the handler's own refusal, since
/// `LogEventConfigurations` carries no modelled length — leaving the stored options
/// exactly as they were.
#[tokio::test]
async fn event_overrides_are_recorded_up_to_the_bound_and_refused_past_it() {
    let overrides = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| event_configuration(&format!("Event{index:04}"), Some("INFO"), None))
                .collect(),
        )
    };

    let at_the_bound = json!({
        "roleArn": ROLE_ARN,
        "eventConfigurations": overrides(MAX_LOG_EVENT_CONFIGURATIONS),
    });
    let emulator = Emulator::new();
    emulator.set_ok("at the bound", &at_the_bound).await;
    assert_reports(&emulator, "at the bound", &at_the_bound).await;

    let already_configured = json!({ "defaultLogLevel": "WARN" });
    let emulator = Emulator::new();
    emulator.set_ok("one past the bound", &already_configured).await;
    let before = emulator.stored();
    let past_the_bound = json!({
        "roleArn": ROLE_ARN,
        "eventConfigurations": overrides(MAX_LOG_EVENT_CONFIGURATIONS + 1),
    });
    let reply = emulator.set_v2_logging_options(&past_the_bound).await;
    assert_eq!(
        reply.status,
        400,
        "case one past the bound: expected SetV2LoggingOptions to refuse with 400, got {} body {}",
        reply.status,
        reply.text()
    );
    assert_eq!(
        reply.error_code(),
        "InvalidRequestException",
        "case one past the bound: expected InvalidRequestException, got {}",
        reply.text()
    );
    assert_eq!(
        emulator.stored(),
        before,
        "case one past the bound: expected the refusal to leave the stored options alone, got {:?}",
        emulator.stored()
    );
}

/// `LogEventConfiguration$eventType` says which event type a configuration is for, so
/// a request naming one event type twice configures it once: the LAST configuration
/// given for an event type is the one the account holds, and the others are reported
/// unchanged.
#[tokio::test]
async fn the_last_configuration_given_for_an_event_type_is_the_one_held() {
    let cases: &[(&str, Value, Value)] = &[
        (
            "the same event type twice, the second naming a destination",
            json!({
                "eventConfigurations": [
                    event_configuration("Connect", Some("DEBUG"), None),
                    event_configuration("Connect", Some("ERROR"), Some("iot/connect-logs")),
                ],
            }),
            json!({
                "eventConfigurations": [
                    event_configuration("Connect", Some("ERROR"), Some("iot/connect-logs")),
                ],
            }),
        ),
        (
            "one event type three times, among others",
            json!({
                "defaultLogLevel": "INFO",
                "eventConfigurations": [
                    event_configuration("Connect", Some("DEBUG"), Some("iot/connect-logs")),
                    event_configuration("Subscribe", Some("WARN"), None),
                    event_configuration("Connect", Some("INFO"), None),
                    event_configuration("Connect", Some("ERROR"), None),
                ],
            }),
            json!({
                "defaultLogLevel": "INFO",
                "eventConfigurations": [
                    event_configuration("Connect", Some("ERROR"), None),
                    event_configuration("Subscribe", Some("WARN"), None),
                ],
            }),
        ),
    ];

    for (case, request, expected) in cases {
        let emulator = Emulator::new();
        emulator.set_ok(case, request).await;
        assert_reports(&emulator, case, expected).await;
    }
}
