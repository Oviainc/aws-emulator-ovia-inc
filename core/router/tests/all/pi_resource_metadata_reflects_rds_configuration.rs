// SPDX-License-Identifier: CC0-1.0

//! Performance Insights features and the two database control planes that own
//! data sources, across services.
//!
//! The rule under test: each reported feature's status is the PROJECTION of the
//! monitoring configuration the service that OWNS the data source persisted for
//! it. `pi_resource_features.rs` pins that for an Amazon RDS owner over the
//! create/modify/delete lifecycle; what this suite adds is the OTHER owner, and
//! the contrast between them in one emulator. Amazon DocumentDB's model has no
//! `DatabaseInsightsMode` member at all, so its control plane records only
//! whether Performance Insights is on — and the two features that depend on a
//! setting it cannot have (`DATABASE_INSIGHTS_ADVANCED_MODE`, and
//! `SQL_LEVEL_STATISTICS`, whose per-SQL metric types
//! `ListAvailableResourceMetrics$MetricTypes` documents for "All engines except
//! for Amazon DocumentDB") are then `UNSUPPORTED` rather than merely off, while
//! the report lifecycle this crate serves follows that one setting. An RDS
//! instance created in the same emulator is the contrast row: the same read
//! reports all three features `ENABLED` for it.
//!
//! Only the WHOLE router can see this: the configuration is persisted by the
//! `docdb` (or `rds`) crate's `CreateDBInstance` and projected by the `pi`
//! crate's `GetResourceMetadata`, and the `pi` crate's own suite can only seed
//! the shared directory by hand — its DocumentDB rows are written, not minted.
//! `pi_data_source_ownership.rs` creates a DocumentDB cluster but asserts the
//! namespace refusals rather than the features.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

use crate::query_wire::{element, form_encode};
use crate::rds_docdb_neptune_signing_name_routing::{authorization, API_VERSION, BOOT_STACK_SIZE};

use service_com_amazonaws_pi_performanceinsightsv20180227::state as pi_state;

/// The `X-Amz-Target` prefix the Performance Insights model answers.
const PI_TARGET_PREFIX: &str = "PerformanceInsightsv20180227";

/// The form-urlencoded body of an awsQuery call, over this test crate's shared
/// percent-encoder rather than another copy of it.
fn query_body(action: &str, params: &[(&str, &str)]) -> bytes::Bytes {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    bytes::Bytes::from(body)
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

/// One DocumentDB instance a row creates, and the statuses Performance Insights
/// must then report for the data source it is.
struct Row {
    case: &'static str,
    cluster: &'static str,
    instance: &'static str,
    /// The `EnablePerformanceInsights` parameter the create carries, or `None` to
    /// leave the member out so the service's own default decides.
    enable_performance_insights: Option<&'static str>,
    /// The expected status of `DATABASE_INSIGHTS_ADVANCED_MODE`,
    /// `PERFORMANCE_ANALYSIS_REPORTS` and `SQL_LEVEL_STATISTICS`, in the order
    /// `REPORTED_FEATURES` lists them.
    expected: [&'static str; 3],
}

#[test]
fn performance_insights_reports_the_features_each_owner_persisted() {
    std::thread::Builder::new()
        .name("pi-owner-resource-features".to_string())
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

            // The rows differ along the one monitoring setting a DocumentDB owner
            // has: Performance Insights on, off, and left to the default.
            let rows: &[Row] = &[
                Row {
                    case: "a DocumentDB instance with Performance Insights on",
                    cluster: "pi-docdb-features-on",
                    instance: "pi-docdb-instance-on",
                    enable_performance_insights: Some("true"),
                    expected: [
                        pi_state::FEATURE_UNSUPPORTED,
                        pi_state::FEATURE_ENABLED,
                        pi_state::FEATURE_UNSUPPORTED,
                    ],
                },
                Row {
                    case: "a DocumentDB instance with Performance Insights off",
                    cluster: "pi-docdb-features-off",
                    instance: "pi-docdb-instance-off",
                    enable_performance_insights: Some("false"),
                    expected: [
                        pi_state::FEATURE_UNSUPPORTED,
                        pi_state::FEATURE_DISABLED,
                        pi_state::FEATURE_UNSUPPORTED,
                    ],
                },
                Row {
                    case: "a DocumentDB instance leaving Performance Insights to the default",
                    cluster: "pi-docdb-features-default",
                    instance: "pi-docdb-instance-default",
                    enable_performance_insights: None,
                    expected: [
                        pi_state::FEATURE_UNSUPPORTED,
                        pi_state::FEATURE_DISABLED,
                        pi_state::FEATURE_UNSUPPORTED,
                    ],
                },
            ];

            for row in rows {
                let case = row.case;
                let (status, rendered) = database(
                    "docdb",
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", row.cluster),
                        ("Engine", "docdb"),
                        ("MasterUsername", "admin"),
                        ("MasterUserPassword", "secret99"),
                    ],
                );
                assert_eq!(
                    status, 200,
                    "case {case}: expected the cluster create to succeed, got {status} with \
                     {rendered}"
                );

                let mut params: Vec<(&str, &str)> = vec![
                    ("DBInstanceIdentifier", row.instance),
                    ("DBClusterIdentifier", row.cluster),
                    ("Engine", "docdb"),
                    ("DBInstanceClass", "db.r5.large"),
                ];
                if let Some(enabled) = row.enable_performance_insights {
                    params.push(("EnablePerformanceInsights", enabled));
                }
                let (status, rendered) = database("docdb", "CreateDBInstance", &params);
                assert_eq!(
                    status, 200,
                    "case {case}: expected the instance create to succeed, got {status} with \
                     {rendered}"
                );
                let dbi_resource_id = element(&rendered, "DbiResourceId");
                assert!(
                    dbi_resource_id.starts_with("db-"),
                    "case {case}: expected a minted DbiResourceId, got {rendered}"
                );

                // The data source is served under the ServiceType of the service
                // that owns it, and every feature's status is the projection of
                // what that service recorded.
                let (status, parsed, rendered) = pi(
                    "GetResourceMetadata",
                    json!({"ServiceType": "DOCDB", "Identifier": dbi_resource_id.as_str()}),
                );
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200 from GetResourceMetadata, got {status} with \
                     {rendered}"
                );
                assert_eq!(
                    parsed.get("Identifier").and_then(Value::as_str),
                    Some(dbi_resource_id.as_str()),
                    "case {case}: expected the addressed Identifier echoed, got {rendered}"
                );
                for (feature, expected) in pi_state::REPORTED_FEATURES.iter().zip(row.expected) {
                    let actual = feature_status(&parsed, feature);
                    assert_eq!(
                        actual, expected,
                        "case {case} / {feature}: expected {expected}, got {actual:?} in \
                         {rendered}"
                    );
                }

                // The SAME id under the service type that does NOT own it names no
                // data source at all, so no feature is reported for it: the
                // directory records DocumentDB as its owner.
                let (status, _, rendered) = pi(
                    "GetResourceMetadata",
                    json!({"ServiceType": "RDS", "Identifier": dbi_resource_id.as_str()}),
                );
                assert_eq!(
                    status, 400,
                    "case {case} under RDS: expected the modelled argument refusal, got {status} \
                     with {rendered}"
                );
                assert!(
                    rendered.contains("InvalidArgumentException"),
                    "case {case} under RDS: expected InvalidArgumentException, got {rendered}"
                );
            }

            // The contrast row, in the same emulator: an Amazon RDS owner DOES
            // have the database-insights mode, so the two features DocumentDB
            // rules out are reported as on for an instance created in advanced
            // mode — the same projection, a different owner.
            let (status, rendered) = database(
                "rds",
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", "pi-owner-rds-advanced"),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.t3.medium"),
                    ("AllocatedStorage", "20"),
                    ("EnablePerformanceInsights", "true"),
                    ("DatabaseInsightsMode", "advanced"),
                ],
            );
            assert_eq!(
                status, 200,
                "case the RDS contrast row: expected the create to succeed, got {status} with \
                 {rendered}"
            );
            let rds_resource_id = element(&rendered, "DbiResourceId");
            let (status, parsed, rendered) = pi(
                "GetResourceMetadata",
                json!({"ServiceType": "RDS", "Identifier": rds_resource_id.as_str()}),
            );
            assert_eq!(
                status, 200,
                "case the RDS contrast row: expected 200, got {status} with {rendered}"
            );
            for feature in pi_state::REPORTED_FEATURES {
                let actual = feature_status(&parsed, feature);
                assert_eq!(
                    actual,
                    pi_state::FEATURE_ENABLED,
                    "case the RDS contrast row / {feature}: expected {}, got {actual:?} in \
                     {rendered}",
                    pi_state::FEATURE_ENABLED
                );
            }
        })
        .expect("spawn the owner resource-features thread")
        .join()
        .expect("Performance Insights must report the features each owner persisted");
}
