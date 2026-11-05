// SPDX-License-Identifier: CC0-1.0

//! Performance Insights features and the database control plane, across two
//! services: the monitoring settings of a DB resource are given to the service
//! that OWNS the resource (`EnablePerformanceInsights` and
//! `DatabaseInsightsMode` are members of Amazon RDS's create and modify
//! requests), and Performance Insights then has to report each feature's status
//! for that resource.
//!
//! This suite needs the WHOLE router: the settings are persisted by one crate and
//! projected onto `GetResourceMetadata$Features` by another, and the point under
//! test is the hand-off — the `pi` crate's own `resource_features` suite can only
//! seed the shared directory by hand.
//!
//! The rule under test: each reported feature's status is the projection of the
//! monitoring configuration the owning service persisted — enabled where the
//! setting it depends on is on, disabled where it is off — and it tracks a later
//! modification of that setting rather than being fixed at create time.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

use crate::rds_docdb_neptune_signing_name_routing::{API_VERSION, BOOT_STACK_SIZE};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix the Performance Insights model answers.
const PI_TARGET_PREFIX: &str = "PerformanceInsightsv20180227";

/// The feature keys `GetResourceMetadata` reports, and the statuses the model's
/// `FeatureStatus` enum defines, as the `pi` crate names them.
use service_com_amazonaws_pi_performanceinsightsv20180227::state as pi_state;

/// A SigV4 credential scope naming `signing_name` in [`REGION`].
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The form-urlencoded body of an awsQuery call.
fn query_body(action: &str, params: &[(&str, &str)]) -> bytes::Bytes {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    body.push(char::from(byte))
                }
                other => body.push_str(&format!("%{other:02X}")),
            }
        }
    }
    bytes::Bytes::from(body)
}

/// The text of the first `<element>` of an awsQuery answer.
fn element(rendered: &str, element: &str) -> String {
    rendered
        .split_once(&format!("<{element}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{element}>")))
        .map(|(text, _)| text.to_string())
        .unwrap_or_default()
}

/// The `Status` of one feature in a `GetResourceMetadata` answer.
fn feature_status(body: &Value, feature: &str) -> String {
    body.get("Features")
        .and_then(|features| features.get(feature))
        .and_then(|metadata| metadata.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One DB instance a row creates, and the feature statuses Performance Insights
/// must report for it.
struct Row {
    case: &'static str,
    instance: &'static str,
    /// The `EnablePerformanceInsights` / `DatabaseInsightsMode` /
    /// `PerformanceInsightsRetentionPeriod` parameters the create carries.
    create_params: &'static [(&'static str, &'static str)],
    /// The expected status of `DATABASE_INSIGHTS_ADVANCED_MODE`,
    /// `PERFORMANCE_ANALYSIS_REPORTS` and `SQL_LEVEL_STATISTICS`, in the order
    /// `REPORTED_FEATURES` lists them.
    expected: [&'static str; 3],
}

#[test]
fn performance_insights_reports_the_features_the_owning_service_persisted() {
    std::thread::Builder::new()
        .name("pi-resource-features".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            // `enable_all`, which is what the emulator's own boot path builds: a DB
            // instance create opens the instance's engine listener through the core, and
            // binding a socket needs the runtime's I/O driver.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            let database = |signing_name: &str, action: &str, params: &[(&str, &str)]| {
                let request = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("authorization", authorization(signing_name))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(query_body(action, params))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                (status, rendered)
            };

            let pi = |operation: &str, input: Value| {
                let request = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("authorization", authorization("pi"))
                    .header("x-amz-target", format!("{PI_TARGET_PREFIX}.{operation}"))
                    .header("content-type", "application/x-amz-json-1.1")
                    .body(bytes::Bytes::from(input.to_string()))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let parsed: Value = serde_json::from_str(&rendered).unwrap_or(Value::Null);
                (status, parsed, rendered)
            };

            // The rows differ along the monitoring configuration the create
            // persists: advanced mode with Performance Insights on (the shape
            // the recorded unit provisions), the default standard mode with it
            // on, and an instance created without it at all.
            let rows: &[Row] = &[
                Row {
                    case: "advanced database-insights mode with Performance Insights on",
                    instance: "pi-features-advanced",
                    create_params: &[
                        ("EnablePerformanceInsights", "true"),
                        ("DatabaseInsightsMode", "advanced"),
                        ("PerformanceInsightsRetentionPeriod", "465"),
                    ],
                    expected: [
                        pi_state::FEATURE_ENABLED,
                        pi_state::FEATURE_ENABLED,
                        pi_state::FEATURE_ENABLED,
                    ],
                },
                Row {
                    case: "Performance Insights on in the default standard mode",
                    instance: "pi-features-standard",
                    create_params: &[("EnablePerformanceInsights", "true")],
                    expected: [
                        pi_state::FEATURE_DISABLED,
                        pi_state::FEATURE_ENABLED,
                        pi_state::FEATURE_DISABLED,
                    ],
                },
                Row {
                    case: "an instance created without Performance Insights",
                    instance: "pi-features-off",
                    create_params: &[],
                    expected: [
                        pi_state::FEATURE_DISABLED,
                        pi_state::FEATURE_DISABLED,
                        pi_state::FEATURE_DISABLED,
                    ],
                },
            ];

            let mut created: Vec<String> = Vec::new();
            for row in rows {
                let mut params: Vec<(&str, &str)> = vec![
                    ("DBInstanceIdentifier", row.instance),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.t3.medium"),
                    ("AllocatedStorage", "20"),
                ];
                params.extend_from_slice(row.create_params);
                let (status, rendered) = database("rds", "CreateDBInstance", &params);
                assert_eq!(
                    status, 200,
                    "case {}: expected the create to succeed, got {status} with {rendered}",
                    row.case
                );
                let dbi_resource_id = element(&rendered, "DbiResourceId");
                assert!(
                    dbi_resource_id.starts_with("db-"),
                    "case {}: expected a minted DbiResourceId, got {rendered}",
                    row.case
                );
                created.push(dbi_resource_id.clone());

                let (status, parsed, rendered) = pi(
                    "GetResourceMetadata",
                    json!({"ServiceType": "RDS", "Identifier": dbi_resource_id.as_str()}),
                );
                assert_eq!(
                    status, 200,
                    "case {}: expected 200 from GetResourceMetadata, got {status} with {rendered}",
                    row.case
                );
                for (feature, expected) in pi_state::REPORTED_FEATURES.iter().zip(row.expected) {
                    let actual = feature_status(&parsed, feature);
                    assert_eq!(
                        actual, expected,
                        "case {} / {feature}: expected {expected}, got {actual:?} in {rendered}",
                        row.case
                    );
                }
            }

            // A newly created DB instance is `creating` for the span the clock
            // owes it, and a modification is refused while it is: poll the owning
            // service for `available` the way a client does, bounded.
            let wait_until_available = |instance: &str| {
                for _ in 0..200 {
                    let (status, rendered) = database(
                        "rds",
                        "DescribeDBInstances",
                        &[("DBInstanceIdentifier", instance)],
                    );
                    assert_eq!(
                        status, 200,
                        "case waiting for {instance}: expected 200, got {status} with {rendered}"
                    );
                    if element(&rendered, "DBInstanceStatus") == "available" {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                panic!("case waiting for {instance}: the instance never reached available");
            };

            // A later modification of the setting moves the status: the
            // instance created in standard mode is moved to advanced mode, and
            // the SAME read reports per-SQL statistics on.
            wait_until_available("pi-features-standard");
            let (status, rendered) = database(
                "rds",
                "ModifyDBInstance",
                &[
                    ("DBInstanceIdentifier", "pi-features-standard"),
                    ("DatabaseInsightsMode", "advanced"),
                    ("EnablePerformanceInsights", "true"),
                    ("PerformanceInsightsRetentionPeriod", "465"),
                ],
            );
            assert_eq!(
                status, 200,
                "case the modify to advanced mode: expected 200, got {status} with {rendered}"
            );
            let modified = created[1].clone();

            let (status, parsed, rendered) = pi(
                "GetResourceMetadata",
                json!({"ServiceType": "RDS", "Identifier": modified.as_str()}),
            );
            assert_eq!(
                status, 200,
                "case after the modify: expected 200, got {status} with {rendered}"
            );
            assert_eq!(
                feature_status(&parsed, pi_state::FEATURE_SQL_LEVEL_STATISTICS),
                pi_state::FEATURE_ENABLED,
                "case after the modify: expected the per-SQL statistics feature enabled, got \
                 {rendered}"
            );

            // Deleting the instance ends the owner's record of it, so the
            // feature statuses become UNDECIDED again rather than staying at
            // the values of a resource that no longer exists.
            wait_until_available("pi-features-advanced");
            let (status, rendered) = database(
                "rds",
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", "pi-features-advanced"),
                    ("SkipFinalSnapshot", "true"),
                ],
            );
            assert_eq!(
                status, 200,
                "case delete the advanced-mode instance: expected 200, got {status} with \
                 {rendered}"
            );
            let deleted = created[0].clone();
            let (status, parsed, rendered) = pi(
                "GetResourceMetadata",
                json!({"ServiceType": "RDS", "Identifier": deleted.as_str()}),
            );
            assert_eq!(
                status, 200,
                "case the deleted instance: expected 200, got {status} with {rendered}"
            );
            assert_eq!(
                feature_status(&parsed, pi_state::FEATURE_SQL_LEVEL_STATISTICS),
                pi_state::FEATURE_UNKNOWN,
                "case the deleted instance: with no configuration on record the status could \
                 not be determined, got {rendered}"
            );
        })
        .expect("spawn the resource-features thread")
        .join()
        .expect("Performance Insights must report the features the owning service persisted");
}
