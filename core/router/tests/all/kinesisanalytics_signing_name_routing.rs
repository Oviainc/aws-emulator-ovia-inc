// SPDX-License-Identifier: CC0-1.0

//! The two Kinesis Analytics API versions share one SigV4 signing name — each request
//! must still be claimed by exactly one of them in the assembled router.
//!
//! AWS publishes the 2015-08-14 (`kinesisanalytics`) and 2018-05-23
//! (`kinesisanalyticsv2`) APIs behind ONE endpoint and ONE signing name: both models
//! declare `endpointPrefix`/`sigv4Name` `kinesisanalytics`
//! (`smithy_query(query="service_info")` on each). The router resolves a request's routing
//! key from the SigV4 credential scope, so a service claiming on that name alone claims
//! the other version's traffic too and the router answers `500 AmbiguousService` — a code
//! no AWS model carries — for every request to BOTH.
//!
//! What names the version is the `awsJson1_1` wire form: a client sends
//! `X-Amz-Target: KinesisAnalytics_20180523.<Operation>` or
//! `KinesisAnalytics_20150814.<Operation>`, and the service-shape prefix differs only by
//! that API-version suffix. So each crate's `can_handle` claims a request whose target
//! names its OWN shape, suffix compared rather than dropped.
//!
//! The rows span what that rule generalises over: the operations each version alone
//! models, the operations BOTH model (where the version in the target is the only
//! difference), the error paths, the fully qualified Coral prefix form of a target, and a
//! target that names no version at all. The service that ANSWERED is read off the answer:
//! the two versions model different outputs for the same operation name (v2's
//! `CreateApplication` returns `ApplicationDetail`, v1's returns `ApplicationSummary`) and
//! hold separate stores, so an application created under one version is absent under the
//! other.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the signing name BOTH versions carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisanalytics/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The `X-Amz-Target` prefix of each API version: its service shape's local name.
const V1_TARGET: &str = "KinesisAnalytics_20150814";
const V2_TARGET: &str = "KinesisAnalytics_20180523";

/// One answer from the assembled router.
struct Answer {
    status: u16,
    /// The awsJson1_1 error code (`__type`), namespace stripped; empty on a success.
    code: String,
    body: Value,
}

/// Call the shared endpoint with `target` naming the operation, as an SDK does.
async fn call(router: &MultiServiceRouter, target: &str, body: Value) -> Answer {
    let host = "kinesisanalytics.us-east-1.amazonaws.com";
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", target)
        .header("content-type", "application/x-amz-json-1.1")
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
    Answer {
        status,
        code,
        body: parsed,
    }
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

/// Neither of the router's own routing failures may appear on a request addressed to the
/// shared endpoint: `AmbiguousService` means both versions claimed it, `UnknownService`
/// that neither did.
fn assert_a_service_answered(case: &str, answer: &Answer) {
    assert_ne!(
        answer.code, "AmbiguousService",
        "case {case:?}: both API versions claimed the request, got HTTP {} ({})",
        answer.status, answer.body
    );
    assert_ne!(
        answer.code, "UnknownService",
        "case {case:?}: neither API version claimed the request, got HTTP {} ({})",
        answer.status, answer.body
    );
}

#[test]
fn each_kinesis_analytics_api_version_answers_its_own_targets() {
    on_a_sized_thread("kinesisanalytics-routing", || {
        Box::pin(async {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let router = build_router(engine);

            // An operation BOTH versions model, whose outputs differ: each version's
            // answer names the version that served it.
            let v2_created = call(
                &router,
                &format!("{V2_TARGET}.CreateApplication"),
                json!({
                    "ApplicationName": "shared-endpoint-app",
                    "RuntimeEnvironment": "FLINK-1_20",
                    "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
                }),
            )
            .await;
            assert_a_service_answered("v2 CreateApplication", &v2_created);
            assert_eq!(
                (v2_created.status, v2_created.code.as_str()),
                (200, ""),
                "case \"v2 CreateApplication\": expected HTTP 200 from the 2018-05-23 \
                 service, got {} {:?} ({})",
                v2_created.status,
                v2_created.code,
                v2_created.body
            );
            assert!(
                v2_created.body["ApplicationDetail"]["ApplicationARN"]
                    .as_str()
                    .is_some_and(|arn| arn.ends_with(":application/shared-endpoint-app")),
                "case \"v2 CreateApplication\": the 2018-05-23 model answers with \
                 ApplicationDetail, got {}",
                v2_created.body
            );

            let v1_created = call(
                &router,
                &format!("{V1_TARGET}.CreateApplication"),
                json!({
                    "ApplicationName": "legacy-sql-app",
                    "ApplicationCode": "SELECT 1;",
                }),
            )
            .await;
            assert_a_service_answered("v1 CreateApplication", &v1_created);
            assert_eq!(
                (v1_created.status, v1_created.code.as_str()),
                (200, ""),
                "case \"v1 CreateApplication\": expected HTTP 200 from the 2015-08-14 \
                 service, got {} {:?} ({})",
                v1_created.status,
                v1_created.code,
                v1_created.body
            );
            assert!(
                v1_created.body["ApplicationSummary"]["ApplicationName"].as_str()
                    == Some("legacy-sql-app"),
                "case \"v1 CreateApplication\": the 2015-08-14 model answers with \
                 ApplicationSummary, got {}",
                v1_created.body
            );

            // Each version answered from its OWN store: the application one version holds
            // is absent under the other version's target, which is the same name on the
            // same endpoint.
            // (case, target prefix, application name, whether that version holds it)
            let reads: [(&str, &str, &str, bool); 4] = [
                ("v2 reads its own application", V2_TARGET, "shared-endpoint-app", true),
                ("v1 reads its own application", V1_TARGET, "legacy-sql-app", true),
                ("v2 does not hold v1's application", V2_TARGET, "legacy-sql-app", false),
                ("v1 does not hold v2's application", V1_TARGET, "shared-endpoint-app", false),
            ];
            for (case, prefix, name, held) in reads {
                let answer = call(
                    &router,
                    &format!("{prefix}.DescribeApplication"),
                    json!({"ApplicationName": name}),
                )
                .await;
                assert_a_service_answered(case, &answer);
                if held {
                    assert_eq!(
                        (answer.status, answer.code.as_str()),
                        (200, ""),
                        "case {case:?}: expected the stored application, got {} {:?} ({})",
                        answer.status,
                        answer.code,
                        answer.body
                    );
                    assert_eq!(
                        answer.body["ApplicationDetail"]["ApplicationName"].as_str(),
                        Some(name),
                        "case {case:?}: expected the application that was created, got {}",
                        answer.body
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.code.as_str()),
                        (400, "ResourceNotFoundException"),
                        "case {case:?}: an application of the other API version must be a \
                         miss, got {} {:?} ({})",
                        answer.status,
                        answer.code,
                        answer.body
                    );
                }
            }

            // The error paths of the version that owns the target, each a modelled 400 and
            // not a routing refusal. The last two operations exist in the 2018-05-23 model
            // only, so a target naming them reaches that version or nothing at all.
            // (case, target, body, expected status, expected code)
            let rows: [(&str, String, Value, u16, &str); 5] = [
                (
                    "v2 CreateApplication with the retired SQL runtime",
                    format!("{V2_TARGET}.CreateApplication"),
                    json!({
                        "ApplicationName": "sql-app",
                        "RuntimeEnvironment": "SQL-1_0",
                        "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
                    }),
                    400,
                    "UnsupportedOperationException",
                ),
                (
                    "v2 CreateApplication with a runtime outside the enum",
                    format!("{V2_TARGET}.CreateApplication"),
                    json!({
                        "ApplicationName": "bad-runtime-app",
                        "RuntimeEnvironment": "FLINK-99_9",
                        "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
                    }),
                    400,
                    "ValidationException",
                ),
                (
                    "v2 CreateApplicationSnapshot of an absent application",
                    format!("{V2_TARGET}.CreateApplicationSnapshot"),
                    json!({"ApplicationName": "no-such-app", "SnapshotName": "snap-1"}),
                    400,
                    "InvalidArgumentException",
                ),
                (
                    "v2 StartApplication with no code content stored",
                    format!("{V2_TARGET}.StartApplication"),
                    json!({"ApplicationName": "shared-endpoint-app"}),
                    400,
                    "InvalidApplicationConfigurationException",
                ),
                (
                    // The fully qualified Coral form of the same target: the service is
                    // the last segment of the prefix, so the version is named either way.
                    "v2 DescribeApplication under the Coral target prefix",
                    format!("com.amazonaws.kinesisanalyticsv2.{V2_TARGET}.DescribeApplication"),
                    json!({"ApplicationName": "no-such-app"}),
                    400,
                    "ResourceNotFoundException",
                ),
            ];

            for (case, target, body, expected_status, expected_code) in rows {
                let answer = call(&router, &target, body).await;
                assert_a_service_answered(case, &answer);
                assert_eq!(
                    (answer.status, answer.code.as_str()),
                    (expected_status, expected_code),
                    "case {case:?}: expected HTTP {expected_status} {expected_code} from the \
                     version the target names, got {} {:?} ({})",
                    answer.status,
                    answer.code,
                    answer.body
                );
                assert!(
                    answer.body.get("Message").is_some() || answer.body.get("message").is_some(),
                    "case {case:?}: a client error carries a message member, got {}",
                    answer.body
                );
            }

            // A target naming NO API version — the suffix that distinguishes them dropped —
            // belongs to neither service. That is the router's `UnknownService`, and
            // deliberately not an ambiguity: picking a version for it would be a guess.
            let unversioned = call(
                &router,
                "KinesisAnalytics.DescribeApplication",
                json!({"ApplicationName": "shared-endpoint-app"}),
            )
            .await;
            assert_ne!(
                unversioned.code, "AmbiguousService",
                "case \"a target naming no API version\": no request may be claimed by both \
                 versions, got HTTP {} ({})",
                unversioned.status, unversioned.body
            );
            assert_eq!(
                unversioned.code, "UnknownService",
                "case \"a target naming no API version\": expected the router's \
                 UnknownService, got HTTP {} {:?} ({})",
                unversioned.status, unversioned.code, unversioned.body
            );
        })
    });
}
