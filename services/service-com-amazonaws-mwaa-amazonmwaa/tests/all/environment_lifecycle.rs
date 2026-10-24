// SPDX-License-Identifier: CC0-1.0

//! The Amazon MWAA environment lifecycle, end to end over HTTP through the router this
//! service mounts in production.
//!
//! The rule every case here asserts: each environment operation answers from the ONE
//! stored environment record — create/update write it, get/list/list-tags read it back,
//! delete removes it — so no response is an echo of the request that asked for it, and
//! the members a request did not carry keep the values already stored. Each table row
//! varies what the rule generalises over (environment class, Airflow version, worker and
//! web-server counts, security-group count, tag count, configured log types, optional S3
//! path/version pairs, access mode, endpoint management); no row is privileged.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region the `AUTHORIZATION` scope above signs for; every ARN and host this service
/// mints for these requests carries it.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than [`state::TRANSITION_SPAN`], so a transition the clock owes has
/// certainly come due.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
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

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
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
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn create(&self, case: &EnvCase) -> Answer {
        self.send(
            "PUT",
            &format!("/environments/{}", case.name),
            Some(case.create_body()),
        )
        .await
    }

    async fn get(&self, name: &str) -> Answer {
        self.send("GET", &format!("/environments/{name}"), None).await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, name: &str) -> Value {
        let answer = self.get(name).await;
        assert_eq!(
            answer.status,
            200,
            "case {name}: expected GetEnvironment 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        answer
            .json()
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("case {name}: GetEnvironment answered without Environment"))
    }

    async fn list(&self, query: &str) -> Answer {
        self.send("GET", &format!("/environments{query}"), None).await
    }

    async fn update(&self, name: &str, body: Value) -> Answer {
        self.send("PATCH", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn delete(&self, name: &str) -> Answer {
        self.send("DELETE", &format!("/environments/{name}"), None)
            .await
    }

    async fn list_tags(&self, arn: &str) -> Answer {
        self.send("GET", &format!("/tags/{}", encode_label(arn)), None)
            .await
    }

    async fn tag(&self, arn: &str, tags: Value) -> Answer {
        self.send(
            "POST",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({"Tags": tags})),
        )
        .await
    }

    async fn untag(&self, arn: &str, keys: &[&str]) -> Answer {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={key}"))
            .collect::<Vec<_>>()
            .join("&");
        self.send(
            "DELETE",
            &format!("/tags/{}?{query}", encode_label(arn)),
            None,
        )
        .await
    }
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// One create request, as a row of the table below. Every field is something the rule
/// generalises over.
struct EnvCase {
    name: &'static str,
    dag_s3_path: &'static str,
    environment_class: Option<&'static str>,
    airflow_version: Option<&'static str>,
    min_workers: Option<i64>,
    max_workers: Option<i64>,
    schedulers: Option<i64>,
    min_webservers: Option<i64>,
    max_webservers: Option<i64>,
    subnets: [&'static str; 2],
    security_groups: &'static [&'static str],
    tags: &'static [(&'static str, &'static str)],
    /// `(LoggingConfiguration member, Enabled, LogLevel)` for each configured log type.
    logging: &'static [(&'static str, bool, &'static str)],
    airflow_options: &'static [(&'static str, &'static str)],
    /// `(member, value)` for the optional S3 path / object-version pairs.
    s3_pairs: &'static [(&'static str, &'static str)],
    webserver_access_mode: Option<&'static str>,
    endpoint_management: Option<&'static str>,
    weekly_window: Option<&'static str>,
    kms_key: Option<&'static str>,
}

impl EnvCase {
    fn create_body(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert(
            "ExecutionRoleArn".into(),
            json!(format!("arn:aws:iam::{ACCOUNT}:role/{}-execution", self.name)),
        );
        body.insert(
            "SourceBucketArn".into(),
            json!("arn:aws:s3:::my-airflow-bucket-unique-name"),
        );
        body.insert("DagS3Path".into(), json!(self.dag_s3_path));
        body.insert(
            "NetworkConfiguration".into(),
            json!({"SubnetIds": self.subnets, "SecurityGroupIds": self.security_groups}),
        );
        for (member, value) in self.s3_pairs {
            body.insert((*member).into(), json!(value));
        }
        if let Some(value) = self.environment_class {
            body.insert("EnvironmentClass".into(), json!(value));
        }
        if let Some(value) = self.airflow_version {
            body.insert("AirflowVersion".into(), json!(value));
        }
        if let Some(value) = self.min_workers {
            body.insert("MinWorkers".into(), json!(value));
        }
        if let Some(value) = self.max_workers {
            body.insert("MaxWorkers".into(), json!(value));
        }
        if let Some(value) = self.schedulers {
            body.insert("Schedulers".into(), json!(value));
        }
        if let Some(value) = self.min_webservers {
            body.insert("MinWebservers".into(), json!(value));
        }
        if let Some(value) = self.max_webservers {
            body.insert("MaxWebservers".into(), json!(value));
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
        if let Some(value) = self.kms_key {
            body.insert("KmsKey".into(), json!(value));
        }
        if !self.tags.is_empty() {
            body.insert("Tags".into(), map_json(self.tags));
        }
        if !self.airflow_options.is_empty() {
            body.insert(
                "AirflowConfigurationOptions".into(),
                map_json(self.airflow_options),
            );
        }
        if !self.logging.is_empty() {
            let mut logging = serde_json::Map::new();
            for (member, enabled, level) in self.logging {
                logging.insert(
                    (*member).into(),
                    json!({"Enabled": enabled, "LogLevel": level}),
                );
            }
            body.insert("LoggingConfiguration".into(), Value::Object(logging));
        }
        Value::Object(body)
    }

    fn arn(&self) -> String {
        format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{}", self.name)
    }

    /// The Airflow version the environment ends up with: the requested one, or the
    /// documented default for an absent `AirflowVersion`.
    fn expected_airflow_version(&self) -> &str {
        self.airflow_version
            .unwrap_or(state::LATEST_AIRFLOW_VERSION)
    }

    /// The class the environment ends up on: the requested one, or the class MWAA assigns
    /// a create that named none.
    fn expected_environment_class(&self) -> &str {
        self.environment_class
            .unwrap_or(state::DEFAULT_ENVIRONMENT_CLASS)
    }

    /// The documented default for `Schedulers` / `MinWebservers` / `MaxWebservers` when
    /// the create names none: 2 for every class except `mw1.micro`, which gets 1.
    fn default_units(&self) -> i64 {
        i64::from(state::default_unit_count(self.expected_environment_class()))
    }

    /// The weekly maintenance window the environment ends up with: the requested one, or
    /// the window MWAA assigns a create that named none.
    fn expected_weekly_window(&self) -> String {
        self.weekly_window.map_or_else(
            || state::default_weekly_maintenance_window_start(self.name),
            str::to_string,
        )
    }
}

fn map_json(pairs: &[(&str, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert((*key).into(), json!(value));
    }
    Value::Object(map)
}

/// The table: one row per create whose members differ along what the rule generalises
/// over. `mw1.micro` is here because its documented default unit count (1) differs from
/// every other class's (2), and the "no optional members at all" row is here because it
/// is the one that exercises every documented default at once.
fn cases() -> Vec<EnvCase> {
    vec![
        EnvCase {
            name: "MyMWAAEnvironment",
            dag_s3_path: "dags",
            environment_class: Some("mw1.small"),
            airflow_version: Some("2.10.3"),
            min_workers: Some(2),
            max_workers: Some(10),
            schedulers: Some(3),
            min_webservers: Some(2),
            max_webservers: Some(4),
            subnets: ["subnet-aaa111", "subnet-bbb222"],
            security_groups: &["sg-111"],
            tags: &[("Environment", "Staging")],
            logging: &[("DagProcessingLogs", true, "INFO")],
            airflow_options: &[],
            s3_pairs: &[],
            webserver_access_mode: Some("PUBLIC_ONLY"),
            endpoint_management: Some("SERVICE"),
            weekly_window: Some("TUE:03:30"),
            kms_key: None,
        },
        EnvCase {
            // Every optional member absent: the row that pins the documented defaults.
            name: "BareDefaults",
            dag_s3_path: "",
            environment_class: None,
            airflow_version: None,
            min_workers: None,
            max_workers: None,
            schedulers: None,
            min_webservers: None,
            max_webservers: None,
            subnets: ["subnet-ccc333", "subnet-ddd444"],
            security_groups: &["sg-222", "sg-333"],
            tags: &[],
            logging: &[],
            airflow_options: &[],
            s3_pairs: &[],
            webserver_access_mode: None,
            endpoint_management: None,
            weekly_window: None,
            kms_key: None,
        },
        EnvCase {
            // mw1.micro: the one class whose documented default unit count is 1.
            name: "Micro_Env-1",
            dag_s3_path: "workflows/dags",
            environment_class: Some("mw1.micro"),
            airflow_version: None,
            min_workers: Some(1),
            max_workers: Some(1),
            schedulers: None,
            min_webservers: None,
            max_webservers: None,
            subnets: ["subnet-eee555", "subnet-fff666"],
            security_groups: &["sg-444"],
            tags: &[("Team", "Data"), ("CostCenter", "4711")],
            logging: &[
                ("DagProcessingLogs", true, "DEBUG"),
                ("SchedulerLogs", false, "WARNING"),
                ("TaskLogs", true, "INFO"),
                ("WebserverLogs", false, "ERROR"),
                ("WorkerLogs", true, "CRITICAL"),
            ],
            airflow_options: &[
                ("core.default_task_retries", "3"),
                ("webserver.dag_default_view", "tree"),
            ],
            s3_pairs: &[
                ("PluginsS3Path", "plugins.zip"),
                ("PluginsS3ObjectVersion", "9VnR0zZr7Qz"),
                ("RequirementsS3Path", "requirements.txt"),
                ("RequirementsS3ObjectVersion", "kQ1x8NnLm2A"),
                ("StartupScriptS3Path", "startup.sh"),
                ("StartupScriptS3ObjectVersion", "aB3dEf5GhI7"),
            ],
            webserver_access_mode: Some("PRIVATE_ONLY"),
            endpoint_management: Some("CUSTOMER"),
            weekly_window: Some("SUN:00:00"),
            kms_key: Some("arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012"),
        },
        EnvCase {
            name: "ZLargeFleet",
            dag_s3_path: "s3/dags/prod",
            environment_class: Some("mw1.2xlarge"),
            airflow_version: Some("2.7.2"),
            min_workers: Some(5),
            max_workers: Some(25),
            schedulers: Some(5),
            min_webservers: Some(3),
            max_webservers: Some(5),
            subnets: ["subnet-ggg777", "subnet-hhh888"],
            security_groups: &["sg-555", "sg-666", "sg-777"],
            tags: &[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")],
            logging: &[("WorkerLogs", true, "WARNING"), ("TaskLogs", false, "INFO")],
            airflow_options: &[("core.parallelism", "64")],
            // The plugins object's version travels with its path: `PluginsS3Path`'s own
            // doc makes the version required whenever the path is specified.
            s3_pairs: &[
                ("PluginsS3Path", "lib/plugins.zip"),
                ("PluginsS3ObjectVersion", "Lm3kQ9pR2sT"),
            ],
            webserver_access_mode: Some("PUBLIC_AND_PRIVATE"),
            endpoint_management: Some("SERVICE"),
            weekly_window: Some("FRI:23:30"),
            kms_key: None,
        },
    ]
}

/// Assert `environment` carries exactly what `case` asked for (and the documented default
/// for every member it did not), plus minted values of the modelled shape.
fn assert_matches_case(case: &EnvCase, environment: &Value) {
    let name = case.name;
    let field = |member: &str| environment.get(member).cloned().unwrap_or(Value::Null);

    assert_eq!(field("Name"), json!(name), "case {name}: Name");
    assert_eq!(field("Arn"), json!(case.arn()), "case {name}: Arn");
    assert_eq!(
        field("DagS3Path"),
        json!(case.dag_s3_path),
        "case {name}: DagS3Path"
    );
    assert_eq!(
        field("SourceBucketArn"),
        json!("arn:aws:s3:::my-airflow-bucket-unique-name"),
        "case {name}: SourceBucketArn"
    );
    assert_eq!(
        field("ExecutionRoleArn"),
        json!(format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution")),
        "case {name}: ExecutionRoleArn"
    );
    assert_eq!(
        field("NetworkConfiguration"),
        json!({"SubnetIds": case.subnets, "SecurityGroupIds": case.security_groups}),
        "case {name}: NetworkConfiguration"
    );
    assert_eq!(
        field("AirflowVersion"),
        json!(case.expected_airflow_version()),
        "case {name}: AirflowVersion"
    );
    assert_eq!(
        field("Schedulers"),
        json!(case.schedulers.unwrap_or_else(|| case.default_units())),
        "case {name}: Schedulers"
    );
    assert_eq!(
        field("MinWebservers"),
        json!(case.min_webservers.unwrap_or_else(|| case.default_units())),
        "case {name}: MinWebservers"
    );
    assert_eq!(
        field("MaxWebservers"),
        json!(case.max_webservers.unwrap_or_else(|| case.default_units())),
        "case {name}: MaxWebservers"
    );
    // Every member MWAA assigns a value for: the requested one, or the service default
    // for a create that named none. None of them is ever absent.
    for (member, expected) in [
        (
            "EnvironmentClass",
            json!(case.expected_environment_class()),
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
            json!(case.expected_weekly_window()),
        ),
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
    ] {
        assert_eq!(field(member), expected, "case {name}: {member}");
    }
    // `KmsKey` is NOT service-assigned: an environment encrypted with an Amazon Web
    // Services owned CMK names no customer key, so an absent one stays absent.
    assert_eq!(
        field("KmsKey"),
        case.kms_key.map_or(Value::Null, |value| json!(value)),
        "case {name}: KmsKey"
    );
    for (member, value) in case.s3_pairs {
        assert_eq!(field(member), json!(value), "case {name}: {member}");
    }
    for member in [
        "PluginsS3Path",
        "PluginsS3ObjectVersion",
        "RequirementsS3Path",
        "RequirementsS3ObjectVersion",
        "StartupScriptS3Path",
        "StartupScriptS3ObjectVersion",
    ] {
        if !case.s3_pairs.iter().any(|(name, _)| *name == member) {
            assert_eq!(field(member), Value::Null, "case {name}: {member}");
        }
    }

    let expected_tags = if case.tags.is_empty() {
        Value::Null
    } else {
        map_json(case.tags)
    };
    assert_eq!(field("Tags"), expected_tags, "case {name}: Tags");

    let expected_options = if case.airflow_options.is_empty() {
        Value::Null
    } else {
        map_json(case.airflow_options)
    };
    assert_eq!(
        field("AirflowConfigurationOptions"),
        expected_options,
        "case {name}: AirflowConfigurationOptions"
    );

    assert_logging(case, &field("LoggingConfiguration"));
    assert_minted(case, environment);
}

/// All five log types read back: a configured one with its own level, an unconfigured one
/// with the service default (off, at the default level). Each carries a CloudWatch
/// log-group ARN exactly when it is enabled.
fn assert_logging(case: &EnvCase, logging: &Value) {
    let name = case.name;
    let configured: Vec<(&str, bool, &str)> = case
        .logging
        .iter()
        .map(|(member, enabled, level)| (*member, *enabled, *level))
        .collect();
    let all_five = [
        "DagProcessingLogs",
        "SchedulerLogs",
        "TaskLogs",
        "WebserverLogs",
        "WorkerLogs",
    ];
    let expected: Vec<(&str, bool, &str)> = all_five
        .iter()
        .map(|member| {
            configured
                .iter()
                .copied()
                .find(|(configured, _, _)| configured == member)
                .unwrap_or((member, false, state::DEFAULT_LOG_LEVEL))
        })
        .collect();
    for (member, enabled, level) in &expected {
        let module = logging
            .get(member)
            .unwrap_or_else(|| panic!("case {name}: LoggingConfiguration.{member} is missing"));
        assert_eq!(
            module.get("Enabled"),
            Some(&json!(enabled)),
            "case {name}: {member}.Enabled"
        );
        assert_eq!(
            module.get("LogLevel"),
            Some(&json!(level)),
            "case {name}: {member}.LogLevel"
        );
        let group = module.get("CloudWatchLogGroupArn").and_then(Value::as_str);
        if *enabled {
            let group = group.unwrap_or_else(|| {
                panic!("case {name}: enabled {member} must report a CloudWatchLogGroupArn")
            });
            let prefix = format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:airflow-{name}-");
            assert!(
                group.starts_with(&prefix),
                "case {name}: {member} log group must start with {prefix}, got {group:?}"
            );
        } else {
            assert_eq!(
                group, None,
                "case {name}: disabled {member} must report no CloudWatchLogGroupArn, got {group:?}"
            );
        }
    }
}

/// The values the SERVICE minted, each matching the shape its modelled member requires.
fn assert_minted(case: &EnvCase, environment: &Value) {
    let name = case.name;
    let text = |member: &str| -> String {
        environment
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name}: {member} is missing"))
            .to_string()
    };

    let url = text("WebserverUrl");
    assert!(
        url.starts_with("https://") && url.ends_with(&format!(".c2.{REGION}.airflow.amazonaws.com")),
        "case {name}: WebserverUrl must be an https MWAA host in {REGION}, got {url:?}"
    );
    assert_eq!(
        text("ServiceRoleArn"),
        format!("arn:aws:iam::{ACCOUNT}:role/aws-service-role/airflow.amazonaws.com/AWSServiceRoleForAmazonMWAA"),
        "case {name}: ServiceRoleArn"
    );
    assert_eq!(
        text("CeleryExecutorQueue"),
        format!("arn:aws:sqs:{REGION}:{ACCOUNT}:airflow-celery-{name}"),
        "case {name}: CeleryExecutorQueue"
    );
    let webserver_vpce = text("WebserverVpcEndpointService");
    let database_vpce = text("DatabaseVpcEndpointService");
    for (member, value) in [
        ("WebserverVpcEndpointService", &webserver_vpce),
        ("DatabaseVpcEndpointService", &database_vpce),
    ] {
        let prefix = format!("com.amazonaws.vpce.{REGION}.vpce-svc-");
        assert!(
            value.starts_with(&prefix),
            "case {name}: {member} must start with {prefix}, got {value:?}"
        );
    }
    assert_ne!(
        webserver_vpce, database_vpce,
        "case {name}: the web-server and database endpoint services must be distinct"
    );
    assert!(
        environment.get("CreatedAt").is_some(),
        "case {name}: CreatedAt must be reported"
    );
}

#[tokio::test]
async fn create_then_get_returns_the_full_persisted_record_for_every_case() {
    let harness = Harness::new();
    for case in cases() {
        let created = harness.create(&case).await;
        assert_eq!(
            created.status,
            200,
            "case {}: expected CreateEnvironment 200, got {} {}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.json().get("Arn").cloned(),
            Some(json!(case.arn())),
            "case {}: CreateEnvironment must answer with the ARN it stored",
            case.name
        );

        // A create answers before the environment is ready: the clock owes AVAILABLE
        // `TRANSITION_SPAN` after the call, so a caller reading straight away sees
        // CREATING, exactly as it does against the real service.
        let environment = harness.environment(case.name).await;
        assert_eq!(
            environment.get("Status").cloned(),
            Some(json!("CREATING")),
            "case {}: a freshly created environment is still CREATING",
            case.name
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    for case in cases() {
        let environment = harness.environment(case.name).await;
        assert_eq!(
            environment.get("Status").cloned(),
            Some(json!("AVAILABLE")),
            "case {}: the clock owes AVAILABLE once {:?} has elapsed",
            case.name,
            state::TRANSITION_SPAN
        );
        assert_eq!(
            environment.get("LastUpdate").cloned(),
            None,
            "case {}: an environment nobody updated reports no LastUpdate",
            case.name
        );
        assert_matches_case(&case, &environment);
    }
}

#[tokio::test]
async fn reading_an_environment_twice_reports_the_same_minted_values() {
    let harness = Harness::new();
    let case = &cases()[0];
    assert_eq!(harness.create(case).await.status, 200, "create must succeed");
    tokio::time::sleep(PAST_TRANSITION).await;

    let first = harness.environment(case.name).await;
    let second = harness.environment(case.name).await;
    for member in [
        "Arn",
        "CreatedAt",
        "WebserverUrl",
        "CeleryExecutorQueue",
        "WebserverVpcEndpointService",
        "DatabaseVpcEndpointService",
        "ServiceRoleArn",
    ] {
        assert_eq!(
            first.get(member),
            second.get(member),
            "case {}: {member} must be stable across reads",
            case.name
        );
    }
}

#[tokio::test]
async fn get_and_the_tag_operations_answer_404_for_an_unknown_resource() {
    let harness = Harness::new();

    let answer = harness.get("NoSuchEnv").await;
    assert_eq!(
        answer.status, 404,
        "GetEnvironment of an unknown name must answer 404, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "GetEnvironment of an unknown name must answer ResourceNotFoundException"
    );

    let answer = harness.delete("NoSuchEnv").await;
    assert_eq!(answer.status, 404, "DeleteEnvironment of an unknown name");
    assert_eq!(answer.error_code(), "ResourceNotFoundException");

    let answer = harness.update("NoSuchEnv", json!({"MaxWorkers": 3})).await;
    assert_eq!(answer.status, 404, "UpdateEnvironment of an unknown name");
    assert_eq!(answer.error_code(), "ResourceNotFoundException");

    let stranger = format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/Nobody");
    for answer in [
        harness.list_tags(&stranger).await,
        harness.tag(&stranger, json!({"k": "v"})).await,
        harness.untag(&stranger, &["k"]).await,
    ] {
        assert_eq!(
            answer.status,
            404,
            "a tag operation on an ARN that resolves to nothing must answer 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "a tag operation on an unknown ARN must answer the wire code the service \
             publishes for its tagging operations"
        );
    }
}

#[tokio::test]
async fn an_idempotent_replay_returns_the_stored_arn_and_a_different_create_is_refused() {
    let harness = Harness::new();
    for case in cases() {
        let first = harness.create(&case).await;
        assert_eq!(first.status, 200, "case {}: first create", case.name);

        // `@idempotent`: the SAME create replays to the SAME stored ARN.
        let replay = harness.create(&case).await;
        assert_eq!(
            replay.status, 200,
            "case {}: replaying an identical create must succeed, got {}",
            case.name, replay.status
        );
        assert_eq!(
            replay.json().get("Arn"),
            first.json().get("Arn"),
            "case {}: a replay must answer the stored ARN",
            case.name
        );

        // A DIFFERENT create of a name that is taken must not clobber the record.
        let mut body = case.create_body();
        body.as_object_mut()
            .expect("an object body")
            .insert("DagS3Path".into(), json!("somewhere/else"));
        let clash = harness
            .send("PUT", &format!("/environments/{}", case.name), Some(body))
            .await;
        assert_eq!(
            clash.status, 400,
            "case {}: a different create of a taken name must be refused, got {} {}",
            case.name,
            clash.status,
            String::from_utf8_lossy(&clash.body)
        );
        assert_eq!(
            clash.error_code(),
            "ValidationException",
            "case {}: a name clash is a ValidationException",
            case.name
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;
    for case in cases() {
        let environment = harness.environment(case.name).await;
        assert_matches_case(&case, &environment);
    }
}

#[tokio::test]
async fn an_update_merges_only_the_members_it_carries() {
    let harness = Harness::new();
    // Each row changes a different member set, so "absent members stay put" is asserted
    // against several shapes of request rather than one.
    let patches: Vec<(&str, Value)> = vec![
        ("MyMWAAEnvironment", json!({"MaxWorkers": 25, "EnvironmentClass": "mw1.medium"})),
        (
            "BareDefaults",
            json!({"AirflowVersion": "2.11.0", "Schedulers": 4, "DagS3Path": "newdags"}),
        ),
        (
            "Micro_Env-1",
            json!({
                "NetworkConfiguration": {"SecurityGroupIds": ["sg-999"]},
                "LoggingConfiguration": {"SchedulerLogs": {"Enabled": true, "LogLevel": "INFO"}},
                "WorkerReplacementStrategy": "GRACEFUL"
            }),
        ),
        (
            "ZLargeFleet",
            json!({
                "AirflowConfigurationOptions": {"core.parallelism": "128"},
                "RequirementsS3Path": "requirements.txt",
                "RequirementsS3ObjectVersion": "zZ9yY8xX7wW",
                "MinWebservers": 2,
                "MaxWebservers": 5
            }),
        ),
    ];

    for case in cases() {
        assert_eq!(
            harness.create(&case).await.status,
            200,
            "case {}: create",
            case.name
        );
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    let mut before: BTreeMap<String, Value> = BTreeMap::new();
    for case in cases() {
        before.insert(case.name.to_string(), harness.environment(case.name).await);
    }

    for (name, patch) in &patches {
        let answer = harness.update(name, patch.clone()).await;
        assert_eq!(
            answer.status,
            200,
            "case {name}: expected UpdateEnvironment 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.json().get("Arn"),
            before[*name].get("Arn"),
            "case {name}: an update keeps the ARN the create stored"
        );

        // The update is in flight: the clock owes AVAILABLE and a SUCCESS LastUpdate.
        let during = harness.environment(name).await;
        assert_eq!(
            during.get("Status").cloned(),
            Some(json!("UPDATING")),
            "case {name}: an environment under update reports UPDATING"
        );
        assert_eq!(
            during.pointer("/LastUpdate/Status").cloned(),
            Some(json!("PENDING")),
            "case {name}: LastUpdate.Status is PENDING while the update is in flight"
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    for (name, patch) in &patches {
        let after = harness.environment(name).await;
        assert_eq!(
            after.get("Status").cloned(),
            Some(json!("AVAILABLE")),
            "case {name}: the clock settles an update to AVAILABLE"
        );
        assert_eq!(
            after.pointer("/LastUpdate/Status").cloned(),
            Some(json!("SUCCESS")),
            "case {name}: LastUpdate.Status settles to SUCCESS"
        );
        assert_eq!(
            after.pointer("/LastUpdate/Source").cloned(),
            Some(json!(state::UPDATE_SOURCE_API)),
            "case {name}: LastUpdate.Source names what drove the change"
        );
        assert!(
            after.pointer("/LastUpdate/CreatedAt").is_some(),
            "case {name}: LastUpdate.CreatedAt must be recorded"
        );

        let patched = patch.as_object().expect("an object patch");
        // Every member the patch carried now reads back as the patch set it.
        for (member, value) in patched {
            match member.as_str() {
                // Recorded on LastUpdate rather than on the environment itself.
                "WorkerReplacementStrategy" => assert_eq!(
                    after.pointer("/LastUpdate/WorkerReplacementStrategy").cloned(),
                    Some(value.clone()),
                    "case {name}: LastUpdate.WorkerReplacementStrategy"
                ),
                // An update may replace the security groups but not the subnets.
                "NetworkConfiguration" => {
                    assert_eq!(
                        after.pointer("/NetworkConfiguration/SecurityGroupIds").cloned(),
                        value.get("SecurityGroupIds").cloned(),
                        "case {name}: an update replaces SecurityGroupIds"
                    );
                    assert_eq!(
                        after.pointer("/NetworkConfiguration/SubnetIds"),
                        before[*name].pointer("/NetworkConfiguration/SubnetIds"),
                        "case {name}: an update leaves SubnetIds alone"
                    );
                }
                // A log type the patch names is replaced; the others are untouched.
                "LoggingConfiguration" => {
                    for (log_type, settings) in value.as_object().expect("a logging object") {
                        assert_eq!(
                            after
                                .pointer(&format!("/LoggingConfiguration/{log_type}/Enabled"))
                                .cloned(),
                            settings.get("Enabled").cloned(),
                            "case {name}: {log_type}.Enabled after the update"
                        );
                        assert_eq!(
                            after
                                .pointer(&format!("/LoggingConfiguration/{log_type}/LogLevel"))
                                .cloned(),
                            settings.get("LogLevel").cloned(),
                            "case {name}: {log_type}.LogLevel after the update"
                        );
                    }
                }
                _ => assert_eq!(
                    after.get(member).cloned(),
                    Some(value.clone()),
                    "case {name}: {member} after the update"
                ),
            }
        }

        // Every OTHER member is exactly what it was before the update.
        let untouched = [
            "Arn",
            "Name",
            "CreatedAt",
            "ExecutionRoleArn",
            "SourceBucketArn",
            "WebserverUrl",
            "CeleryExecutorQueue",
            "ServiceRoleArn",
            "Tags",
            "KmsKey",
            "WeeklyMaintenanceWindowStart",
            "WebserverAccessMode",
            "EndpointManagement",
            "MinWorkers",
            "MaxWorkers",
            "EnvironmentClass",
            "AirflowVersion",
            "DagS3Path",
            "Schedulers",
            "MinWebservers",
            "MaxWebservers",
            "PluginsS3Path",
            "StartupScriptS3Path",
            "AirflowConfigurationOptions",
        ];
        for member in untouched {
            if patched.contains_key(member) {
                continue;
            }
            assert_eq!(
                after.get(member),
                before[*name].get(member),
                "case {name}: {member} was not in the patch and must be unchanged"
            );
        }
    }
}

#[tokio::test]
async fn list_environments_names_exactly_what_is_stored_and_pages_through_it() {
    let harness = Harness::new();
    let mut expected: Vec<String> = Vec::new();
    for case in cases() {
        assert_eq!(
            harness.create(&case).await.status,
            200,
            "case {}: create",
            case.name
        );
        expected.push(case.name.to_string());
    }
    expected.sort();

    let answer = harness.list("").await;
    assert_eq!(answer.status, 200, "ListEnvironments must answer 200");
    assert_eq!(
        answer.json().get("Environments").cloned(),
        Some(json!(expected)),
        "an unpaged list names exactly the stored environments, sorted"
    );
    assert_eq!(
        answer.json().get("NextToken").cloned(),
        None,
        "a list that fits in one page carries no NextToken"
    );

    // Every page size from 1 up past the number stored: paging must visit each name
    // exactly once, in the same order, whatever the page size.
    for page_size in 1..=(expected.len() + 1) {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(value) => format!("?MaxResults={page_size}&NextToken={value}"),
                None => format!("?MaxResults={page_size}"),
            };
            let answer = harness.list(&query).await;
            assert_eq!(
                answer.status,
                200,
                "page size {page_size}: expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let page = answer.json();
            let names: Vec<String> = page
                .get("Environments")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("page size {page_size}: Environments is required"))
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                names.len() <= page_size,
                "page size {page_size}: a page must hold at most {page_size} names, got {}",
                names.len()
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "page size {page_size}: paging must terminate, got {pages} pages"
            );
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: paging must visit every stored name exactly once, in order"
        );
    }

    // A token this service never minted names no position to resume from.
    let answer = harness.list("?NextToken=not-a-token").await;
    assert_eq!(
        answer.status, 400,
        "a NextToken this service never minted must be refused, got {}",
        answer.status
    );
    assert_eq!(answer.error_code(), "ValidationException");

    // `MaxResults` is `@range(1, 25)`; the boundary validator refuses anything outside it.
    for out_of_range in [0, 26] {
        let answer = harness.list(&format!("?MaxResults={out_of_range}")).await;
        assert_eq!(
            answer.status, 400,
            "MaxResults={out_of_range} is outside @range(1, 25) and must be refused, got {}",
            answer.status
        );
    }
}

#[tokio::test]
async fn tags_read_and_write_the_same_map_the_environment_reports() {
    let harness = Harness::new();
    for case in cases() {
        assert_eq!(
            harness.create(&case).await.status,
            200,
            "case {}: create",
            case.name
        );
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    for case in cases() {
        let arn = case.arn();
        let mut expected: BTreeMap<String, String> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        // What the create seeded is what ListTagsForResource reports.
        let listed = harness.list_tags(&arn).await;
        assert_eq!(listed.status, 200, "case {}: ListTagsForResource", case.name);
        assert_eq!(
            listed.json().get("Tags").cloned(),
            tags_json(&expected),
            "case {}: ListTagsForResource reports the tags the create seeded",
            case.name
        );

        // Adding: a new key appears, and an existing key is overwritten.
        let added = json!({"Team": "Platform", "Owner": "sre"});
        assert_eq!(
            harness.tag(&arn, added.clone()).await.status,
            200,
            "case {}: TagResource",
            case.name
        );
        expected.insert("Team".to_string(), "Platform".to_string());
        expected.insert("Owner".to_string(), "sre".to_string());
        assert_eq!(
            harness.list_tags(&arn).await.json().get("Tags").cloned(),
            tags_json(&expected),
            "case {}: TagResource writes the map ListTagsForResource reads",
            case.name
        );
        assert_eq!(
            harness.environment(case.name).await.get("Tags").cloned(),
            tags_json(&expected),
            "case {}: GetEnvironment reports the SAME tag map",
            case.name
        );

        // Removing: only the named keys go, and a key that is not there is not an error.
        assert_eq!(
            harness.untag(&arn, &["Team", "NeverSet"]).await.status,
            200,
            "case {}: UntagResource",
            case.name
        );
        expected.remove("Team");
        assert_eq!(
            harness.list_tags(&arn).await.json().get("Tags").cloned(),
            tags_json(&expected),
            "case {}: UntagResource removes exactly the keys it named",
            case.name
        );

        // Emptying: `TagMap` is @length(1, 50), so no `Tags` member is reported at all.
        let remaining: Vec<&str> = expected.keys().map(String::as_str).collect();
        assert_eq!(
            harness.untag(&arn, &remaining).await.status,
            200,
            "case {}: UntagResource of every key",
            case.name
        );
        assert_eq!(
            harness.list_tags(&arn).await.json().get("Tags").cloned(),
            None,
            "case {}: an untagged environment reports no Tags member",
            case.name
        );
        assert_eq!(
            harness.environment(case.name).await.get("Tags").cloned(),
            None,
            "case {}: GetEnvironment agrees that the environment is untagged",
            case.name
        );
    }
}

/// The modelled `Tags` member for a tag map: absent when the map is empty, because
/// `TagMap` is `@length(1, 50)`.
fn tags_json(tags: &BTreeMap<String, String>) -> Option<Value> {
    if tags.is_empty() {
        return None;
    }
    let mut map = serde_json::Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), json!(value));
    }
    Some(Value::Object(map))
}

#[tokio::test]
async fn delete_removes_the_environment_from_every_reader() {
    let harness = Harness::new();
    for case in cases() {
        assert_eq!(
            harness.create(&case).await.status,
            200,
            "case {}: create",
            case.name
        );
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    let mut remaining: Vec<String> = cases()
        .iter()
        .map(|case| case.name.to_string())
        .collect();
    remaining.sort();

    for case in cases() {
        let arn = case.arn();
        let deleted = harness.delete(case.name).await;
        assert_eq!(
            deleted.status,
            200,
            "case {}: expected DeleteEnvironment 200, got {} {}",
            case.name,
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        assert!(
            deleted.body.is_empty() || deleted.json() == json!({}),
            "case {}: DeleteEnvironment answers an empty body, got {}",
            case.name,
            String::from_utf8_lossy(&deleted.body)
        );

        let gone = harness.get(case.name).await;
        assert_eq!(
            gone.status, 404,
            "case {}: GetEnvironment after a delete must answer 404",
            case.name
        );
        assert_eq!(gone.error_code(), "ResourceNotFoundException");

        // The tags went with the record, so its ARN resolves to nothing.
        let tags = harness.list_tags(&arn).await;
        assert_eq!(
            tags.status, 404,
            "case {}: the deleted environment's ARN must resolve to nothing",
            case.name
        );

        // And a second delete of the same name is a 404, not a silent success.
        let again = harness.delete(case.name).await;
        assert_eq!(
            again.status, 404,
            "case {}: a second DeleteEnvironment must answer 404",
            case.name
        );

        remaining.retain(|name| name != case.name);
        let listed = harness.list("").await;
        assert_eq!(
            listed.json().get("Environments").cloned(),
            Some(json!(remaining)),
            "case {}: ListEnvironments must no longer name a deleted environment",
            case.name
        );
    }
}

#[tokio::test]
async fn a_create_without_subnets_or_security_groups_is_refused() {
    let harness = Harness::new();
    // `NetworkConfiguration` is @required but both of its members are optional in the
    // model, so these are refusals the handler owns rather than boundary ones.
    let bad: Vec<(&str, Value)> = vec![
        ("NoNetwork", json!({})),
        (
            "NoSubnets",
            json!({"SecurityGroupIds": ["sg-111"]}),
        ),
        (
            "NoGroups",
            json!({"SubnetIds": ["subnet-aaa111", "subnet-bbb222"]}),
        ),
    ];
    for (name, network) in bad {
        let body = json!({
            "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/x"),
            "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
            "DagS3Path": "dags",
            "NetworkConfiguration": network,
        });
        let answer = harness
            .send("PUT", &format!("/environments/{name}"), Some(body))
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {name}: a create without subnets or security groups must be refused, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {name}: the refusal is a ValidationException"
        );
        assert_eq!(
            harness.get(name).await.status,
            404,
            "case {name}: a refused create must store nothing"
        );
    }
}
