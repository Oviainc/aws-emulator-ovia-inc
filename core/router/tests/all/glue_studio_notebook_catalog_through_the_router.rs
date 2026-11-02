// SPDX-License-Identifier: CC0-1.0

//! A Kinesis Analytics V2 Studio notebook and the Glue Data Catalog it names, driven
//! through the ASSEMBLED emulator — the router every service is mounted in.
//!
//! The dispatch rule under test: a request to an endpoint whose SigV4 signing name
//! several mounted API versions share is dispatched to the version its `awsJson1_1`
//! `X-Amz-Target` service-shape prefix names, API-version suffix included, and is
//! answered by that version's own model — never by the router's `AmbiguousService`
//! (several services claimed it) or `UnknownService` (none did). Both Kinesis Analytics
//! models declare `endpointPrefix`/`sigv4Name` `kinesisanalytics`
//! (`smithy_query(query="service_info")` on each), so a mounted version claiming on the
//! signing name alone claims its peer's traffic too and the assembled router answers
//! every request to BOTH with a code no AWS model carries — which makes every
//! cross-service workflow over this surface unreachable before its first call lands.
//!
//! What that unreachable workflow is, and the second rule here: a write that persists or
//! re-uses a notebook's `GlueDataCatalogConfiguration` resolves the database its
//! `DatabaseARN` names in the Glue Data Catalog AS IT STANDS at that moment, and is
//! refused `InvalidArgumentException` (HTTP 400), persisting nothing, when the catalog
//! holds no such database; a READ of the notebook reports the stored ARN whether or not it
//! still resolves. Glue's own `DeleteDatabase` is not blocked by a notebook naming the
//! database, so the notebook is stranded — every update refused — until the database is
//! re-created under that name.
//!
//! The rows span what the catalog rule generalises over: the form and case of the
//! database name, and the state of the catalog at each write (held, never held, dropped,
//! re-created). The routing rows span what the dispatch rule generalises over: each
//! version's own target prefix, the fully qualified Coral form of one, another service's
//! signing name in the same router, and a target naming no version at all.
//!
//! Assembling the router costs stack, so each check runs on an explicitly sized thread
//! for the same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call here is signed for, and so the one the ARNs carry.
const REGION: &str = "us-east-1";

/// The account id this emulator mints its ARNs under, and the catalog a `DatabaseARN`
/// has to name for the notebook's own Data Catalog to be the one resolved.
const ACCOUNT: &str = "000000000000";

/// The target prefix of each Kinesis Analytics API version: its service shape's local
/// name, which differs only by the API-version suffix.
const V1_TARGET: &str = "KinesisAnalytics_20150814";
const V2_TARGET: &str = "KinesisAnalytics_20180523";

/// The signing name BOTH Kinesis Analytics versions declare.
const KDA_SIGNING_NAME: &str = "kinesisanalytics";

/// The notebook runtime and the mode a Studio notebook runs in
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")`: the `ZEPPELIN-FLINK-`
/// family, run `INTERACTIVE`).
const NOTEBOOK_RUNTIME: &str = "ZEPPELIN-FLINK-3_0";

/// The log level a notebook that configured none reports, so the level a refused update
/// must leave in place.
const DEFAULT_LOG_LEVEL: &str = "INFO";

/// One answer from the assembled router.
struct Answer {
    status: u16,
    /// The `awsJson1_1` error code (`__type`), namespace stripped; empty on a success.
    code: String,
    /// The error message, in either of the members this emulator's envelopes carry it in:
    /// a modelled exception declares `Message`, a boundary refusal `message`.
    message: String,
    body: Value,
}

/// Call one `awsJson1_1` operation of the assembled emulator the way an SDK does: `POST /`
/// with the credential scope naming `signing_name` and `X-Amz-Target` naming the service
/// shape and the operation.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    body: &Value,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        code,
        message,
        body: parsed,
    }
}

/// Neither of the router's own routing failures may answer a request addressed to a
/// service this emulator mounts: `AmbiguousService` means several claimed it,
/// `UnknownService` that none did. Every call of both suites goes through this.
fn assert_one_service_answered(case: &str, answer: &Answer) {
    assert_ne!(
        answer.code, "AmbiguousService",
        "case {case:?}: more than one mounted service claimed the request, got HTTP {} ({})",
        answer.status, answer.body
    );
    assert_ne!(
        answer.code, "UnknownService",
        "case {case:?}: no mounted service claimed the request, got HTTP {} ({})",
        answer.status, answer.body
    );
}

/// Call one Kinesis Analytics V2 operation, asserting a service — exactly one — answered.
async fn kda(router: &MultiServiceRouter, case: &str, op: &str, body: &Value) -> Answer {
    let answer = call(
        router,
        KDA_SIGNING_NAME,
        &format!("{V2_TARGET}.{op}"),
        body,
    )
    .await;
    assert_one_service_answered(&format!("{case}: {op}"), &answer);
    answer
}

/// Call one Glue operation, asserting a service — exactly one — answered.
async fn glue(router: &MultiServiceRouter, case: &str, op: &str, body: &Value) -> Answer {
    let answer = call(router, "glue", &format!("AWSGlue.{op}"), body).await;
    assert_one_service_answered(&format!("{case}: {op}"), &answer);
    answer
}

/// The ARN of a database of this emulator's own Data Catalog.
fn database_arn(database: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT}:database/{database}")
}

/// The whole `Message` a catalog reference resolving to no database is refused with. The
/// fixed text is written out here, so this suite pins the wording instead of reading the
/// service's own constant; the one variable part is the database name the request names.
fn missing_database_message(database: &str) -> String {
    format!("Cannot find database with name '{database}' in Glue Data Catalog")
}

/// The whole `Message` Glue refuses a read of a database it does not hold with.
fn glue_database_not_found_message(database: &str) -> String {
    format!("Database {database} not found.")
}

/// A `CreateApplication` body for a Studio notebook whose catalog names `arn`.
fn notebook_body(name: &str, arn: &str) -> Value {
    json!({
        "ApplicationName": name,
        "RuntimeEnvironment": NOTEBOOK_RUNTIME,
        "ApplicationMode": "INTERACTIVE",
        "ServiceExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/kda-service-role"),
        "ApplicationConfiguration": {
            "ZeppelinApplicationConfiguration": {
                "CatalogConfiguration": {
                    "GlueDataCatalogConfiguration": {"DatabaseARN": arn}
                }
            }
        }
    })
}

/// The configuration-only update both phases of a row send, byte for byte identical.
fn log_level_update(name: &str, current_version: i64, level: &str) -> Value {
    json!({
        "ApplicationName": name,
        "CurrentApplicationVersionId": current_version,
        "ApplicationConfigurationUpdate": {
            "ZeppelinApplicationConfigurationUpdate": {
                "MonitoringConfigurationUpdate": {"LogLevelUpdate": level}
            }
        }
    })
}

/// The `DatabaseARN` a described notebook reports for its catalog.
fn described_database_arn(detail: &Value) -> Option<&str> {
    detail
        .pointer(
            "/ApplicationConfigurationDescription/ZeppelinApplicationConfigurationDescription\
             /CatalogConfigurationDescription/GlueDataCatalogConfigurationDescription/DatabaseARN",
        )
        .and_then(Value::as_str)
}

/// The Zeppelin log level a described notebook reports.
fn described_log_level(detail: &Value) -> Option<&str> {
    detail
        .pointer(
            "/ApplicationConfigurationDescription/ZeppelinApplicationConfigurationDescription\
             /MonitoringConfigurationDescription/LogLevel",
        )
        .and_then(Value::as_str)
}

/// Describe a notebook, asserting the read succeeded, and return its `ApplicationDetail`.
async fn describe(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let answer = kda(
        router,
        case,
        "DescribeApplication",
        &json!({"ApplicationName": name}),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (200, ""),
        "case {case:?}: DescribeApplication({name:?}) must answer the stored notebook, \
         got {} {:?} ({})",
        answer.status,
        answer.code,
        answer.body
    );
    answer.body["ApplicationDetail"].clone()
}

/// Run one async check on a thread sized like `boot_serve.rs`'s.
fn on_a_sized_thread(
    name: &str,
    check: fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
) {
    std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(check());
        })
        .expect("spawn the check thread")
        .join()
        .expect("the check must pass");
}

/// One row of the catalog table: the name the database is WRITTEN under and the name the
/// notebook's `DatabaseARN` names it by.
struct CatalogRow {
    case: &'static str,
    /// The `Name` Glue's `CreateDatabase` writes.
    created_as: &'static str,
    /// The database name the notebook's ARN carries, which the catalog resolves.
    named_as: &'static str,
}

#[test]
fn a_studio_notebook_tracks_the_glue_database_it_names() {
    on_a_sized_thread("glue-studio-notebook-catalog", || {
        Box::pin(async {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let router = build_router(engine);

            let rows = [
                CatalogRow {
                    case: "a database named as it was created",
                    created_as: "probe_db",
                    named_as: "probe_db",
                },
                CatalogRow {
                    // The catalog keys a database by its folded name, so a notebook
                    // resolves it under either spelling.
                    case: "a database named in another case than it was created in",
                    created_as: "MixedCase_DB",
                    named_as: "mixedcase_db",
                },
                CatalogRow {
                    case: "a database name carrying digits and underscores",
                    created_as: "metrics_db_2024",
                    named_as: "metrics_db_2024",
                },
            ];

            for (index, row) in rows.iter().enumerate() {
                let CatalogRow {
                    case,
                    created_as,
                    named_as,
                } = row;
                let notebook = format!("studio-notebook-{index}");

                // The catalog holds the database the notebook is about to name.
                let created = glue(
                    &router,
                    case,
                    "CreateDatabase",
                    &json!({"DatabaseInput": {"Name": created_as}}),
                )
                .await;
                assert_eq!(
                    (created.status, created.code.as_str()),
                    (200, ""),
                    "case {case:?}: CreateDatabase({created_as:?}) must answer 200, got {} {:?} ({})",
                    created.status,
                    created.code,
                    created.body
                );

                // A notebook naming a database the catalog does NOT hold is refused, and
                // nothing is persisted for it.
                let absent = format!("{named_as}_absent");
                let refused = kda(
                    &router,
                    case,
                    "CreateApplication",
                    &notebook_body(&format!("absent-catalog-{index}"), &database_arn(&absent)),
                )
                .await;
                assert_eq!(
                    (refused.status, refused.code.as_str()),
                    (400, "InvalidArgumentException"),
                    "case {case:?}: a notebook naming a database the catalog does not hold \
                     must be refused, got {} {:?} ({})",
                    refused.status,
                    refused.code,
                    refused.body
                );
                assert_eq!(
                    refused.message,
                    missing_database_message(&absent),
                    "case {case:?}: expected the catalog refusal's wording, got {}",
                    refused.body
                );
                let missing = kda(
                    &router,
                    case,
                    "DescribeApplication",
                    &json!({"ApplicationName": format!("absent-catalog-{index}")}),
                )
                .await;
                assert_eq!(
                    (missing.status, missing.code.as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case:?}: a refused create must leave no application behind, \
                     got {} {:?} ({})",
                    missing.status,
                    missing.code,
                    missing.body
                );

                // The notebook whose catalog DOES resolve is created, at its first version.
                let arn = database_arn(named_as);
                let accepted = kda(
                    &router,
                    case,
                    "CreateApplication",
                    &notebook_body(&notebook, &arn),
                )
                .await;
                assert_eq!(
                    (accepted.status, accepted.code.as_str()),
                    (200, ""),
                    "case {case:?}: a notebook naming a database the catalog holds must be \
                     created, got {} {:?} ({})",
                    accepted.status,
                    accepted.code,
                    accepted.body
                );
                let detail = describe(&router, case, &notebook).await;
                assert_eq!(
                    described_database_arn(&detail),
                    Some(arn.as_str()),
                    "case {case:?}: expected the stored DatabaseARN, got {detail}"
                );
                assert_eq!(
                    detail["ApplicationVersionId"].as_i64(),
                    Some(1),
                    "case {case:?}: a created notebook is at its first version, got {detail}"
                );

                // Glue drops the database while the notebook names it: the delete is not
                // blocked, and Glue then holds nothing under that name.
                let dropped = glue(&router, case, "DeleteDatabase", &json!({"Name": named_as})).await;
                assert_eq!(
                    (dropped.status, dropped.code.as_str()),
                    (200, ""),
                    "case {case:?}: a notebook naming the database must not block its \
                     delete, got {} {:?} ({})",
                    dropped.status,
                    dropped.code,
                    dropped.body
                );
                let gone = glue(&router, case, "GetDatabase", &json!({"Name": named_as})).await;
                assert_eq!(
                    (gone.status, gone.code.as_str()),
                    (400, "EntityNotFoundException"),
                    "case {case:?}: the dropped database must be a catalog miss, got {} {:?} ({})",
                    gone.status,
                    gone.code,
                    gone.body
                );
                assert_eq!(
                    gone.message,
                    glue_database_not_found_message(named_as),
                    "case {case:?}: expected Glue's own miss wording, got {}",
                    gone.body
                );

                // The READ of the notebook still reports the now-dangling ARN, unchanged:
                // reporting the stored configuration is not re-resolving it.
                let stranded = describe(&router, case, &notebook).await;
                assert_eq!(
                    described_database_arn(&stranded),
                    Some(arn.as_str()),
                    "case {case:?}: a dropped database leaves the stored ARN as it was, \
                     got {stranded}"
                );
                assert_eq!(
                    stranded["ApplicationVersionId"].as_i64(),
                    Some(1),
                    "case {case:?}: reading a stranded notebook moves no version, \
                     got {stranded}"
                );

                // While the database is absent, even a configuration-only update is
                // refused — and changes nothing.
                let update = log_level_update(&notebook, 1, "WARN");
                let blocked = kda(&router, case, "UpdateApplication", &update).await;
                assert_eq!(
                    (blocked.status, blocked.code.as_str()),
                    (400, "InvalidArgumentException"),
                    "case {case:?}: an update re-using an unresolvable catalog must be \
                     refused, got {} {:?} ({})",
                    blocked.status,
                    blocked.code,
                    blocked.body
                );
                assert_eq!(
                    blocked.message,
                    missing_database_message(named_as),
                    "case {case:?}: expected the catalog refusal's wording, got {}",
                    blocked.body
                );
                let untouched = describe(&router, case, &notebook).await;
                assert_eq!(
                    (
                        untouched["ApplicationVersionId"].as_i64(),
                        described_log_level(&untouched)
                    ),
                    (Some(1), Some(DEFAULT_LOG_LEVEL)),
                    "case {case:?}: a refused update moves neither the version nor the log \
                     level, got {untouched}"
                );

                // Re-created under the same name, the IDENTICAL update is accepted and
                // mints the next version.
                let recreated = glue(
                    &router,
                    case,
                    "CreateDatabase",
                    &json!({"DatabaseInput": {"Name": named_as}}),
                )
                .await;
                assert_eq!(
                    (recreated.status, recreated.code.as_str()),
                    (200, ""),
                    "case {case:?}: CreateDatabase({named_as:?}) must answer 200, got {} {:?} ({})",
                    recreated.status,
                    recreated.code,
                    recreated.body
                );
                let applied = kda(&router, case, "UpdateApplication", &update).await;
                assert_eq!(
                    (applied.status, applied.code.as_str()),
                    (200, ""),
                    "case {case:?}: the identical update must be accepted once the database \
                     is back, got {} {:?} ({})",
                    applied.status,
                    applied.code,
                    applied.body
                );
                assert_eq!(
                    applied.body["ApplicationDetail"]["ApplicationVersionId"].as_i64(),
                    Some(2),
                    "case {case:?}: an accepted update mints the next version, got {}",
                    applied.body
                );
                let updated = describe(&router, case, &notebook).await;
                assert_eq!(
                    (
                        updated["ApplicationVersionId"].as_i64(),
                        described_log_level(&updated)
                    ),
                    (Some(2), Some("WARN")),
                    "case {case:?}: expected the minted version and the updated log level, \
                     got {updated}"
                );
                assert_eq!(
                    described_database_arn(&updated),
                    Some(arn.as_str()),
                    "case {case:?}: the catalog no update named must survive it, got {updated}"
                );
            }
        })
    });
}

#[test]
fn glue_and_both_kinesis_analytics_versions_each_answer_their_own_targets() {
    on_a_sized_thread("glue-kinesisanalytics-dispatch", || {
        Box::pin(async {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let router = build_router(engine);

            let case = "one signing name, two API versions, one workflow";
            let database = "shared_target_db";
            let created = glue(
                &router,
                case,
                "CreateDatabase",
                &json!({"DatabaseInput": {"Name": database}}),
            )
            .await;
            assert_eq!(
                (created.status, created.code.as_str()),
                (200, ""),
                "case {case:?}: CreateDatabase must answer 200, got {} {:?} ({})",
                created.status,
                created.code,
                created.body
            );

            // Another service's signing name in the SAME router is claimed by that
            // service, which answers from its own store.
            let read = glue(&router, case, "GetDatabase", &json!({"Name": database})).await;
            assert_eq!(
                read.body["Database"]["Name"].as_str(),
                Some(database),
                "case {case:?}: expected the database Glue holds, got {}",
                read.body
            );

            // The 2018-05-23 target creates the Studio notebook.
            let shared_name = "shared-target-app";
            let arn = database_arn(database);
            let v2 = kda(
                &router,
                case,
                "CreateApplication",
                &notebook_body(shared_name, &arn),
            )
            .await;
            assert_eq!(
                (v2.status, v2.code.as_str()),
                (200, ""),
                "case {case:?}: the 2018-05-23 target must create the notebook, got {} {:?} ({})",
                v2.status,
                v2.code,
                v2.body
            );
            assert!(
                v2.body["ApplicationDetail"]["ApplicationARN"]
                    .as_str()
                    .is_some_and(|arn| arn.ends_with(&format!(":application/{shared_name}"))),
                "case {case:?}: the 2018-05-23 model answers with ApplicationDetail, got {}",
                v2.body
            );

            // The SAME application name under the 2015-08-14 target, on the same endpoint
            // and the same signing name: the other version holds its own store, so this is
            // a create and not a collision — and its answer is that version's own output
            // shape (`ApplicationSummary`, where the 2018-05-23 model answers
            // `ApplicationDetail`), which is what names the version that served it.
            let v1 = call(
                &router,
                KDA_SIGNING_NAME,
                &format!("{V1_TARGET}.CreateApplication"),
                &json!({"ApplicationName": shared_name, "ApplicationCode": "SELECT 1;"}),
            )
            .await;
            assert_one_service_answered(&format!("{case}: v1 CreateApplication"), &v1);
            assert_eq!(
                (v1.status, v1.code.as_str()),
                (200, ""),
                "case {case:?}: the 2015-08-14 target must be served by that version, \
                 got {} {:?} ({})",
                v1.status,
                v1.code,
                v1.body
            );
            assert_eq!(
                v1.body["ApplicationSummary"]["ApplicationName"].as_str(),
                Some(shared_name),
                "case {case:?}: the 2015-08-14 model answers with ApplicationSummary, got {}",
                v1.body
            );

            // The notebook the 2018-05-23 version holds is untouched by its peer's write,
            // read back under both forms of ITS target: the short prefix an SDK sends and
            // the fully qualified Coral prefix, whose last segment names the same version.
            for target in [
                format!("{V2_TARGET}.DescribeApplication"),
                format!("com.amazonaws.kinesisanalyticsv2.{V2_TARGET}.DescribeApplication"),
            ] {
                let answer = call(
                    &router,
                    KDA_SIGNING_NAME,
                    &target,
                    &json!({"ApplicationName": shared_name}),
                )
                .await;
                assert_one_service_answered(&format!("{case}: {target}"), &answer);
                assert_eq!(
                    (answer.status, answer.code.as_str()),
                    (200, ""),
                    "case {case:?}: {target} must read the 2018-05-23 notebook, got {} {:?} ({})",
                    answer.status,
                    answer.code,
                    answer.body
                );
                assert_eq!(
                    described_database_arn(&answer.body["ApplicationDetail"]),
                    Some(arn.as_str()),
                    "case {case:?}: {target} must answer the notebook's own catalog, got {}",
                    answer.body
                );
            }

            // A target naming NO API version — the suffix that distinguishes them dropped —
            // belongs to neither version. That is the router's `UnknownService`, and
            // deliberately not an ambiguity: picking a version for it would be a guess.
            let unversioned = call(
                &router,
                KDA_SIGNING_NAME,
                "KinesisAnalytics.DescribeApplication",
                &json!({"ApplicationName": shared_name}),
            )
            .await;
            assert_ne!(
                unversioned.code, "AmbiguousService",
                "case {case:?}: no request may be claimed by both versions, got HTTP {} ({})",
                unversioned.status, unversioned.body
            );
            assert_eq!(
                unversioned.code, "UnknownService",
                "case {case:?}: a target naming no API version is claimed by neither, \
                 got HTTP {} {:?} ({})",
                unversioned.status, unversioned.code, unversioned.body
            );
        })
    });
}
