// SPDX-License-Identifier: CC0-1.0

//! Performance Insights and the database control planes, across THREE services: a DB
//! resource is minted by Amazon RDS or Amazon DocumentDB, and Performance Insights
//! then has to decide whether the `ServiceType` a request names is the service that
//! OWNS the resource its `Identifier` names.
//!
//! This suite needs the WHOLE router: the resource id is minted by one crate and
//! judged by another, and the point under test is that the two agree about who owns
//! it — a single-service test can only seed the shared directory by hand (the `pi`
//! crate's own `service_type_namespace` suite does exactly that), never prove the
//! hand-off.
//!
//! The rule under test: an identifier that a database service recorded as ITS data
//! source is served under that service's `ServiceType` and refused under the other
//! with the modelled `InvalidArgumentException` (HTTP 400), whose message names the
//! engine the resource runs; an identifier no database service recorded stays
//! undecided and is served under either; and once the resource is deleted, its owner
//! stops claiming the id, so the refusal stops too. The identifier FORM decides
//! nothing: both services mint `db-<TOKEN>` instance ids.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

use crate::rds_docdb_neptune_signing_name_routing::{API_VERSION, BOOT_STACK_SIZE};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix the Performance Insights model answers.
const PI_TARGET_PREFIX: &str = "PerformanceInsightsv20180227";

/// A SigV4 credential scope naming `signing_name` in [`REGION`] — the signal the
/// router picks the owning service by.
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

/// The wire error code of an awsJson answer, without its namespace prefix.
fn error_code(body: &Value) -> String {
    let raw = body.get("__type").and_then(Value::as_str).unwrap_or_default();
    raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
}

/// The error message of an awsJson answer.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[test]
fn performance_insights_serves_a_data_source_only_under_the_service_that_owns_it() {
    std::thread::Builder::new()
        .name("pi-data-source-ownership".to_string())
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

            // An awsQuery call to one of the database control planes.
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

            // An awsJson1_1 call to Performance Insights.
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

            // 1. Amazon RDS mints a DB instance, which records its `DbiResourceId` as
            //    an RDS data source running the engine the create asked for.
            let (status, rendered) = database(
                "rds",
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", "pi-owner-mysql"),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.t3.micro"),
                    ("AllocatedStorage", "20"),
                    ("EnablePerformanceInsights", "true"),
                ],
            );
            assert_eq!(
                status, 200,
                "case create the RDS instance: expected 200, got {status} with {rendered}"
            );
            let rds_instance = element(&rendered, "DbiResourceId");
            assert!(
                rds_instance.starts_with("db-"),
                "case create the RDS instance: expected a minted DbiResourceId, got {rendered}"
            );

            // 2. Amazon DocumentDB mints a cluster, recording its own resource id.
            let (status, rendered) = database(
                "docdb",
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", "pi-owner-docdb"),
                    ("Engine", "docdb"),
                    ("MasterUsername", "admin"),
                    ("MasterUserPassword", "secret99"),
                ],
            );
            assert_eq!(
                status, 200,
                "case create the DocumentDB cluster: expected 200, got {status} with {rendered}"
            );
            let docdb_cluster = element(&rendered, "DbClusterResourceId");
            assert!(
                docdb_cluster.starts_with("cluster-"),
                "case create the DocumentDB cluster: expected a minted DbClusterResourceId, \
                 got {rendered}"
            );

            // Each row: the data source, the ServiceType that owns it, the engine the
            // refusal must name, and the ServiceType that does NOT own it.
            let owned: [(&str, &str, &str, &str, &str); 2] = [
                (
                    "the RDS MySQL instance",
                    rds_instance.as_str(),
                    "RDS",
                    "mysql",
                    "DOCDB",
                ),
                (
                    "the DocumentDB cluster",
                    docdb_cluster.as_str(),
                    "DOCDB",
                    "docdb",
                    "RDS",
                ),
            ];

            for (case, identifier, owner, engine, other) in owned {
                for operation in ["ListPerformanceAnalysisReports", "GetResourceMetadata"] {
                    // The owner serves it.
                    let (status, parsed, rendered) = pi(
                        operation,
                        json!({"ServiceType": owner, "Identifier": identifier}),
                    );
                    assert_eq!(
                        status, 200,
                        "case {case} under its owner {owner} / {operation}: expected 200, got \
                         {status} with {rendered}"
                    );
                    let _ = parsed;

                    // The other service type refuses it as the modelled argument
                    // error, naming the engine the stored resource runs.
                    let (status, parsed, rendered) = pi(
                        operation,
                        json!({"ServiceType": other, "Identifier": identifier}),
                    );
                    assert_eq!(
                        (status, error_code(&parsed).as_str()),
                        (400, "InvalidArgumentException"),
                        "case {case} under {other} / {operation}: expected the modelled 400, got \
                         {status} with {rendered}"
                    );
                    assert!(
                        error_message(&parsed).contains(engine),
                        "case {case} under {other} / {operation}: expected the refusal to name \
                         the engine {engine}, got {rendered}"
                    );
                }
            }

            // GetResourceMetadata echoes the identifier it was asked about.
            let (_, parsed, rendered) = pi(
                "GetResourceMetadata",
                json!({"ServiceType": "RDS", "Identifier": rds_instance.as_str()}),
            );
            assert_eq!(
                parsed.get("Identifier").and_then(Value::as_str),
                Some(rds_instance.as_str()),
                "case the RDS instance's metadata: expected its identifier echoed, got {rendered}"
            );

            // An identifier no database service minted here is UNDECIDED, so both
            // service types still serve it: a missing record is never a refusal.
            for service_type in ["RDS", "DOCDB"] {
                let (status, _, rendered) = pi(
                    "ListPerformanceAnalysisReports",
                    json!({
                        "ServiceType": service_type,
                        "Identifier": "db-ABCDEFGHIJKLMNOPQRSTU1VW2X",
                    }),
                );
                assert_eq!(
                    status, 200,
                    "case an unrecorded identifier under {service_type}: expected 200, got \
                     {status} with {rendered}"
                );
            }

            // Deleting the instance ends RDS's claim on the id, so Performance
            // Insights stops refusing it under the other service type: the ownership
            // answer tracks the resource rather than being minted once and kept.
            let (status, rendered) = database(
                "rds",
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", "pi-owner-mysql"),
                    ("SkipFinalSnapshot", "true"),
                ],
            );
            assert_eq!(
                status, 200,
                "case delete the RDS instance: expected 200, got {status} with {rendered}"
            );
            let (status, _, rendered) = pi(
                "ListPerformanceAnalysisReports",
                json!({"ServiceType": "DOCDB", "Identifier": rds_instance.as_str()}),
            );
            assert_eq!(
                status, 200,
                "case the deleted instance's id under DOCDB: with no owner on record the pair is \
                 undecided again, expected 200, got {status} with {rendered}"
            );

            // The DocumentDB cluster is untouched by all of that: still owned, still
            // refused under RDS.
            let (status, parsed, rendered) = pi(
                "GetResourceMetadata",
                json!({"ServiceType": "RDS", "Identifier": docdb_cluster.as_str()}),
            );
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidArgumentException"),
                "case the DocumentDB cluster under RDS after the RDS delete: expected the \
                 modelled 400, got {status} with {rendered}"
            );
        })
        .expect("spawn the data-source ownership thread")
        .join()
        .expect("Performance Insights must serve a data source only under its owning service");
}
