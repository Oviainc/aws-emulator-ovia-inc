// SPDX-License-Identifier: CC0-1.0

//! Service-assigned defaults on `CreateEnvironment`, read back through `GetEnvironment`.
//!
//! The rule: a create resolves every environment member the service assigns a value for
//! — the request's value when it carries one, the service's own default when it does not —
//! and stores the resolved value, so no member Amazon MWAA reports for every environment
//! is ever absent from a later read.
//!
//! The table below varies what the rule generalises over: which of the defaulted members
//! the request omits (all of them, none of them, one at a time), the environment class the
//! defaults are resolved against, and the log types the request configures. The catalogs
//! the rows draw from (`EnvironmentClass`'s valid-value list, `WebserverAccessMode`,
//! `EndpointManagement` and `LoggingLevel`'s enum values) come from this service's Smithy
//! model and are named on each constant below.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router resolves this service by its SigV4
/// credential scope, as an SDK request does. Signature verification is off in this harness.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// Every `EnvironmentClass` the member's own doc lists as valid ("Valid values: mw1.micro,
/// mw1.small, mw1.medium, mw1.large, mw1.xlarge, and mw1.2xlarge"), from
/// `smithy_query(query="shape", shape="Environment")` → `EnvironmentClass`.
const ENVIRONMENT_CLASSES: [&str; 6] = [
    "mw1.micro",
    "mw1.small",
    "mw1.medium",
    "mw1.large",
    "mw1.xlarge",
    "mw1.2xlarge",
];

/// Every `WebserverAccessMode` value, from
/// `smithy_query(query="shape", shape="WebserverAccessMode")`.
const WEBSERVER_ACCESS_MODES: [&str; 3] = ["PRIVATE_ONLY", "PUBLIC_AND_PRIVATE", "PUBLIC_ONLY"];

/// Every `EndpointManagement` value, from
/// `smithy_query(query="shape", shape="EndpointManagement")`.
const ENDPOINT_MANAGEMENT_MODES: [&str; 2] = ["CUSTOMER", "SERVICE"];

/// Every `LoggingLevel` value, from `smithy_query(query="shape", shape="LoggingLevel")`.
const LOGGING_LEVELS: [&str; 5] = ["CRITICAL", "ERROR", "WARNING", "INFO", "DEBUG"];

/// The five `LoggingConfiguration` module members, from
/// `smithy_query(query="shape", shape="LoggingConfiguration")`.
const LOG_TYPES: [&str; 5] = [
    "DagProcessingLogs",
    "SchedulerLogs",
    "TaskLogs",
    "WebserverLogs",
    "WorkerLogs",
];

/// Comfortably longer than [`state::TRANSITION_SPAN`], so a transition the clock owes an
/// environment has certainly come due — an `UpdateEnvironment` is accepted only once the
/// environment has settled into AVAILABLE.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The days `WeeklyMaintenanceWindowStart`'s pattern accepts
/// (`(MON|TUE|WED|THU|FRI|SAT|SUN):([01]\d|2[0-3]):(00|30)`).
const WINDOW_DAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

/// The emulator, assembled as the router assembles it: one shared engine, one service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body().to_vec();
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON body, got {error}: {}",
                    String::from_utf8_lossy(&body)
                )
            })
        };
        (status, json)
    }

    async fn create(&self, name: &str, body: Value) -> (u16, Value) {
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn update(&self, name: &str, body: Value) -> (u16, Value) {
        self.send("PATCH", &format!("/environments/{name}"), Some(body))
            .await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, name: &str) -> Value {
        let (status, json) = self.send("GET", &format!("/environments/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {name}: expected GetEnvironment 200, got {status} {json}"
        );
        json.get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("case {name}: GetEnvironment answered without Environment"))
    }
}

/// One create request: the required members, plus whichever defaulted members this row
/// chooses to name.
struct Case {
    name: &'static str,
    environment_class: Option<&'static str>,
    min_workers: Option<i64>,
    max_workers: Option<i64>,
    webserver_access_mode: Option<&'static str>,
    endpoint_management: Option<&'static str>,
    weekly_window: Option<&'static str>,
    /// `(LoggingConfiguration member, Enabled, LogLevel)` for each configured log type.
    logging: Vec<(&'static str, bool, &'static str)>,
}

impl Case {
    /// A row that names NONE of the defaulted members.
    fn bare(name: &'static str) -> Self {
        Self {
            name,
            environment_class: None,
            min_workers: None,
            max_workers: None,
            webserver_access_mode: None,
            endpoint_management: None,
            weekly_window: None,
            logging: Vec::new(),
        }
    }

    fn create_body(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert(
            "ExecutionRoleArn".into(),
            json!("arn:aws:iam::000000000000:role/mwaa-exec"),
        );
        body.insert(
            "SourceBucketArn".into(),
            json!("arn:aws:s3:::my-airflow-bucket-unique-name"),
        );
        body.insert("DagS3Path".into(), json!("dags"));
        body.insert(
            "NetworkConfiguration".into(),
            json!({
                "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
                "SecurityGroupIds": ["sg-00000000000000001"],
            }),
        );
        if let Some(value) = self.environment_class {
            body.insert("EnvironmentClass".into(), json!(value));
        }
        if let Some(value) = self.min_workers {
            body.insert("MinWorkers".into(), json!(value));
        }
        if let Some(value) = self.max_workers {
            body.insert("MaxWorkers".into(), json!(value));
        }
        if let Some(value) = self.webserver_access_mode {
            body.insert("WebserverAccessMode".into(), json!(value));
        }
        if let Some(value) = self.endpoint_management {
            body.insert("EndpointManagement".into(), json!(value));
        }
        if let Some(value) = self.weekly_window {
            body.insert("WeeklyMaintenanceWindowStart".into(), json!(value));
        }
        if !self.logging.is_empty() {
            let mut logging = serde_json::Map::new();
            for (member, enabled, level) in &self.logging {
                logging.insert(
                    (*member).into(),
                    json!({"Enabled": enabled, "LogLevel": level}),
                );
            }
            body.insert("LoggingConfiguration".into(), Value::Object(logging));
        }
        Value::Object(body)
    }
}

/// The table: the bare create, a create naming every defaulted member, one row per
/// environment class (so the sizing defaults are resolved against each class the model
/// lists), one row per access mode and endpoint-management value, and rows configuring one
/// log type at a time at a different level.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case::bare("BareDefaults"),
        Case {
            name: "EverythingNamed",
            environment_class: Some("mw1.large"),
            min_workers: Some(3),
            max_workers: Some(17),
            webserver_access_mode: Some("PRIVATE_ONLY"),
            endpoint_management: Some("CUSTOMER"),
            weekly_window: Some("WED:13:00"),
            logging: LOG_TYPES
                .iter()
                .zip(LOGGING_LEVELS)
                .map(|(log_type, level)| (*log_type, true, level))
                .collect(),
        },
    ];
    // One class at a time, with every other defaulted member omitted: the class a create
    // names must not change which members get a default.
    for (index, class) in ENVIRONMENT_CLASSES.iter().enumerate() {
        cases.push(Case {
            environment_class: Some(class),
            ..Case::bare(CLASS_CASE_NAMES[index])
        });
    }
    // One access mode / endpoint-management value at a time: a named value is kept, and
    // every other defaulted member still gets its default.
    for (index, mode) in WEBSERVER_ACCESS_MODES.iter().enumerate() {
        cases.push(Case {
            webserver_access_mode: Some(mode),
            ..Case::bare(ACCESS_MODE_CASE_NAMES[index])
        });
    }
    for (index, mode) in ENDPOINT_MANAGEMENT_MODES.iter().enumerate() {
        cases.push(Case {
            endpoint_management: Some(mode),
            ..Case::bare(ENDPOINT_CASE_NAMES[index])
        });
    }
    // One log type at a time, each at a different level: the four the request leaves alone
    // still read back with the service default.
    for (index, log_type) in LOG_TYPES.iter().enumerate() {
        cases.push(Case {
            logging: vec![(*log_type, index % 2 == 0, LOGGING_LEVELS[index])],
            ..Case::bare(LOG_TYPE_CASE_NAMES[index])
        });
    }
    // Worker counts named one side at a time: the other side still gets its default.
    cases.push(Case {
        min_workers: Some(4),
        ..Case::bare("MinWorkersOnly")
    });
    cases.push(Case {
        max_workers: Some(40),
        ..Case::bare("MaxWorkersOnly")
    });
    cases.push(Case {
        weekly_window: Some("SUN:00:30"),
        ..Case::bare("WindowOnly")
    });
    cases
}

/// `EnvironmentName` is `^[a-zA-Z][0-9a-zA-Z-_]*$`, so each row carries a literal name
/// rather than one built from a value that holds a dot.
const CLASS_CASE_NAMES: [&str; 6] = [
    "ClassMicro",
    "ClassSmall",
    "ClassMedium",
    "ClassLarge",
    "ClassXlarge",
    "Class2xlarge",
];
/// See [`CLASS_CASE_NAMES`].
const ACCESS_MODE_CASE_NAMES: [&str; 3] = ["AccessPrivate", "AccessBoth", "AccessPublic"];
/// See [`CLASS_CASE_NAMES`].
const ENDPOINT_CASE_NAMES: [&str; 2] = ["EndpointCustomer", "EndpointService"];
/// See [`CLASS_CASE_NAMES`].
const LOG_TYPE_CASE_NAMES: [&str; 5] = [
    "LogDagProcessing",
    "LogScheduler",
    "LogTask",
    "LogWebserver",
    "LogWorker",
];

/// What the environment must report for every member the service assigns a value for.
fn assert_defaults_resolved(case: &Case, environment: &Value) {
    let name = case.name;
    let field = |member: &str| environment.get(member).cloned().unwrap_or(Value::Null);
    let class = case
        .environment_class
        .unwrap_or(state::DEFAULT_ENVIRONMENT_CLASS);
    let units = i64::from(state::default_unit_count(class));

    for (member, expected) in [
        ("EnvironmentClass", json!(class)),
        (
            "MinWorkers",
            json!(case
                .min_workers
                .unwrap_or_else(|| i64::from(state::DEFAULT_MIN_WORKERS))),
        ),
        (
            "MaxWorkers",
            json!(case
                .max_workers
                .unwrap_or_else(|| i64::from(state::DEFAULT_MAX_WORKERS))),
        ),
        (
            "WebserverAccessMode",
            json!(case
                .webserver_access_mode
                .unwrap_or(state::DEFAULT_WEBSERVER_ACCESS_MODE)),
        ),
        (
            "EndpointManagement",
            json!(case
                .endpoint_management
                .unwrap_or(state::DEFAULT_ENDPOINT_MANAGEMENT)),
        ),
        (
            "WeeklyMaintenanceWindowStart",
            json!(case.weekly_window.map_or_else(
                || state::default_weekly_maintenance_window_start(name),
                str::to_string
            )),
        ),
        // The three sizing members whose default the class decides.
        ("Schedulers", json!(units)),
        ("MinWebservers", json!(units)),
        ("MaxWebservers", json!(units)),
    ] {
        assert_eq!(
            field(member),
            expected,
            "case {name}: {member} must report the requested value or the service default, got {:?}",
            field(member)
        );
    }

    assert_window_shape(name, &field("WeeklyMaintenanceWindowStart"));

    // All five log types, always: a configured one with its own settings, the rest with
    // the service default (not enabled, at the default level, no log group).
    let logging = field("LoggingConfiguration");
    for log_type in LOG_TYPES {
        let module = logging.get(log_type).cloned().unwrap_or_else(|| {
            panic!("case {name}: LoggingConfiguration.{log_type} must be reported, got {logging:?}")
        });
        let (expected_enabled, expected_level) = case
            .logging
            .iter()
            .find(|(configured, _, _)| *configured == log_type)
            .map_or((false, state::DEFAULT_LOG_LEVEL), |(_, enabled, level)| {
                (*enabled, *level)
            });
        assert_eq!(
            module.get("Enabled").cloned(),
            Some(json!(expected_enabled)),
            "case {name}: {log_type}.Enabled, got {module:?}"
        );
        assert_eq!(
            module.get("LogLevel").cloned(),
            Some(json!(expected_level)),
            "case {name}: {log_type}.LogLevel, got {module:?}"
        );
        let group = module.get("CloudWatchLogGroupArn").and_then(Value::as_str);
        assert_eq!(
            group.is_some(),
            expected_enabled,
            "case {name}: {log_type} reports a CloudWatchLogGroupArn exactly when enabled, got {group:?}"
        );
    }
}

/// A reported window lies inside `WeeklyMaintenanceWindowStart`'s own pattern: a day it
/// names, an hour from 00 to 23, and a 30-minute boundary.
fn assert_window_shape(case: &str, window: &Value) {
    let text = window
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: WeeklyMaintenanceWindowStart must be a string, got {window:?}"));
    let parts: Vec<&str> = text.split(':').collect();
    assert_eq!(
        parts.len(),
        3,
        "case {case}: a window is DAY:HH:MM, got {text:?}"
    );
    assert!(
        WINDOW_DAYS.contains(&parts[0]),
        "case {case}: window day must be one of {WINDOW_DAYS:?}, got {:?}",
        parts[0]
    );
    assert_eq!(
        parts[1].len(),
        2,
        "case {case}: window hour is two digits, got {:?}",
        parts[1]
    );
    let hour: u32 = parts[1]
        .parse()
        .unwrap_or_else(|_| panic!("case {case}: window hour must be numeric, got {:?}", parts[1]));
    assert!(
        hour <= 23,
        "case {case}: window hour must be 00..=23, got {hour}"
    );
    assert!(
        parts[2] == "00" || parts[2] == "30",
        "case {case}: a window starts on a 30-minute boundary, got {:?}",
        parts[2]
    );
}

#[tokio::test]
async fn a_create_resolves_every_service_assigned_member_for_every_case() {
    let harness = Harness::new();
    for case in cases() {
        let (status, json) = harness.create(case.name, case.create_body()).await;
        assert_eq!(
            status, 200,
            "case {}: expected CreateEnvironment 200, got {status} {json}",
            case.name
        );
        let environment = harness.environment(case.name).await;
        assert_defaults_resolved(&case, &environment);
    }
}

#[tokio::test]
async fn a_defaulted_member_is_stable_across_reads_and_survives_an_unrelated_update() {
    let harness = Harness::new();
    let case = Case::bare("StableDefaults");
    let (status, json) = harness.create(case.name, case.create_body()).await;
    assert_eq!(
        status, 200,
        "case {}: create must succeed, got {status} {json}",
        case.name
    );

    let first = harness.environment(case.name).await;
    let second = harness.environment(case.name).await;
    for member in [
        "EnvironmentClass",
        "MinWorkers",
        "MaxWorkers",
        "WebserverAccessMode",
        "EndpointManagement",
        "WeeklyMaintenanceWindowStart",
        "LoggingConfiguration",
    ] {
        assert_eq!(
            first.get(member),
            second.get(member),
            "case {}: {member} must be the same on a second read, got {:?} then {:?}",
            case.name,
            first.get(member),
            second.get(member)
        );
    }

    // An update that names none of them leaves every default exactly where the create put
    // it; one that names a member replaces only that member. Each update waits for the
    // environment to settle: an environment still CREATING, or under an update already in
    // flight, does not accept another one.
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, json) = harness
        .update(case.name, json!({"DagS3Path": "other/dags"}))
        .await;
    assert_eq!(
        status, 200,
        "case {}: unrelated update must succeed, got {status} {json}",
        case.name
    );
    let after = harness.environment(case.name).await;
    assert_defaults_resolved(&case, &after);

    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, json) = harness
        .update(
            case.name,
            json!({
                "EnvironmentClass": "mw1.medium",
                "MinWorkers": 2,
                "MaxWorkers": 12,
                "WebserverAccessMode": "PRIVATE_ONLY",
                "WeeklyMaintenanceWindowStart": "MON:01:30",
                "LoggingConfiguration": {"TaskLogs": {"Enabled": true, "LogLevel": "DEBUG"}},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {}: update must succeed, got {status} {json}",
        case.name
    );
    let after = harness.environment(case.name).await;
    // `Schedulers` / `MinWebservers` / `MaxWebservers` keep what the create resolved: the
    // update named none of them, and an update merges only the members it carries.
    for member in ["Schedulers", "MinWebservers", "MaxWebservers"] {
        assert_eq!(
            after.get(member),
            first.get(member),
            "case {}: {member} was not in the patch and must be unchanged, got {:?}",
            case.name,
            after.get(member)
        );
    }
    let field = |member: &str| after.get(member).cloned().unwrap_or(Value::Null);
    assert_eq!(
        field("EnvironmentClass"),
        json!("mw1.medium"),
        "case {}: an update replaces EnvironmentClass",
        case.name
    );
    for (member, expected) in [
        ("MinWorkers", json!(2)),
        ("MaxWorkers", json!(12)),
        ("WebserverAccessMode", json!("PRIVATE_ONLY")),
        ("WeeklyMaintenanceWindowStart", json!("MON:01:30")),
        (
            "EndpointManagement",
            json!(state::DEFAULT_ENDPOINT_MANAGEMENT),
        ),
    ] {
        assert_eq!(
            field(member),
            expected,
            "case {}: {member} after the update",
            case.name
        );
    }
    // Only `TaskLogs` was patched; the other four keep the service default the create
    // resolved for them.
    let patched_logging = [("TaskLogs", true, "DEBUG")];
    for log_type in LOG_TYPES {
        let (expected_enabled, expected_level) = patched_logging
            .iter()
            .find(|(configured, _, _)| *configured == log_type)
            .map_or((false, state::DEFAULT_LOG_LEVEL), |(_, enabled, level)| {
                (*enabled, *level)
            });
        assert_eq!(
            after
                .pointer(&format!("/LoggingConfiguration/{log_type}/Enabled"))
                .cloned(),
            Some(json!(expected_enabled)),
            "case {}: {log_type}.Enabled after the update",
            case.name
        );
        assert_eq!(
            after
                .pointer(&format!("/LoggingConfiguration/{log_type}/LogLevel"))
                .cloned(),
            Some(json!(expected_level)),
            "case {}: {log_type}.LogLevel after the update",
            case.name
        );
    }
}

#[tokio::test]
async fn the_assigned_maintenance_window_differs_between_environments() {
    let harness = Harness::new();
    let names = [
        "WindowOne",
        "WindowTwo",
        "WindowThree",
        "WindowFour",
        "WindowFive",
        "WindowSix",
        "WindowSeven",
        "WindowEight",
    ];
    let mut windows: BTreeSet<String> = BTreeSet::new();
    for name in names {
        let case = Case::bare(name);
        let (status, json) = harness.create(name, case.create_body()).await;
        assert_eq!(
            status, 200,
            "case {name}: create must succeed, got {status} {json}"
        );
        let environment = harness.environment(name).await;
        let window = environment
            .get("WeeklyMaintenanceWindowStart")
            .cloned()
            .unwrap_or(Value::Null);
        assert_window_shape(name, &window);
        windows.insert(window.as_str().unwrap_or_default().to_string());
    }
    assert!(
        windows.len() > 1,
        "the window MWAA assigns must not be one fixed value for every environment, got {windows:?}"
    );
}
