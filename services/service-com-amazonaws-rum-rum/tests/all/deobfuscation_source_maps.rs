// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the conditional requirement a deobfuscation configuration
//! carries: source-map unminification can only be turned ON for a monitor that names the
//! bucket the maps are read from.
//!
//! From the model (`smithy_query(query="shape", shape="JavaScriptSourceMaps")` against
//! `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`),
//! member `S3Uri`: "The S3Uri of the bucket or folder that stores the source map files. It
//! is required if status is ENABLED." `S3Uri` is an OPTIONAL member carrying only a
//! `@length` (1..1024) and a `@pattern`, so an absent value satisfies every modelled
//! constraint and the requirement is prose no boundary walk can apply — which makes it the
//! handler's, for EVERY operation that accepts the member.
//!
//! The table below spans the class rather than the one configuration a report cited: both
//! statuses `DeobfuscationStatus` admits (`DISABLED`, `ENABLED` — source:
//! `smithy_query(query="shape", shape="DeobfuscationStatus")`), with and without a bucket,
//! bucket URIs of three different forms (bare bucket, bucket and key prefix, dotted bucket
//! name with a trailing slash), and a configuration that names no `JavaScriptSourceMaps` at
//! all. Each row is driven through `CreateAppMonitor` AND through `UpdateAppMonitor`,
//! because the rule is the member's and not one operation's, and the update also has to
//! leave the stored record untouched when it refuses.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator, get_app_monitor, message, Reply};

/// The error code the refusal carries: the `ValidationException` both app-monitor writes
/// declare (`@httpError(400)`).
const VALIDATION_CODE: &str = "ValidationException";

/// The whole message the refusal is stated in, spelled out here rather than read back out
/// of the crate so the test pins the wording instead of agreeing with it by construction.
const REFUSAL_MESSAGE: &str = "S3Uri is required when JavaScriptSourceMaps Status is ENABLED.";

/// One row of the table: a `DeobfuscationConfiguration` (or none at all), and whether the
/// documented requirement refuses it.
struct Case {
    label: &'static str,
    /// The `DeobfuscationConfiguration` the request carries, or `None` for a request that
    /// names the member nowhere.
    configuration: Option<Value>,
    refused: bool,
}

/// Every configuration the rule has to decide, spanning the two statuses, the presence of
/// a bucket, three spellings of a bucket URI and a configuration that names no source-map
/// structure.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "enabled with no bucket",
            configuration: Some(json!({"JavaScriptSourceMaps": {"Status": "ENABLED"}})),
            refused: true,
        },
        Case {
            label: "enabled with a bare bucket",
            configuration: Some(json!({
                "JavaScriptSourceMaps": {"Status": "ENABLED", "S3Uri": "s3://maps-bucket"}
            })),
            refused: false,
        },
        Case {
            label: "enabled with a bucket and a key prefix",
            configuration: Some(json!({
                "JavaScriptSourceMaps": {"Status": "ENABLED", "S3Uri": "s3://maps-bucket/app"}
            })),
            refused: false,
        },
        Case {
            label: "enabled with a dotted bucket and a nested folder",
            configuration: Some(json!({
                "JavaScriptSourceMaps": {
                    "Status": "ENABLED",
                    "S3Uri": "s3://rum.source.maps/app/v2/",
                }
            })),
            refused: false,
        },
        Case {
            label: "disabled with no bucket",
            configuration: Some(json!({"JavaScriptSourceMaps": {"Status": "DISABLED"}})),
            refused: false,
        },
        Case {
            // The model conditions the requirement on the status alone, so a bucket kept
            // beside a disabled status breaks nothing.
            label: "disabled with a bucket",
            configuration: Some(json!({
                "JavaScriptSourceMaps": {"Status": "DISABLED", "S3Uri": "s3://maps-bucket/app"}
            })),
            refused: false,
        },
        Case {
            label: "a configuration naming no source maps",
            configuration: Some(json!({})),
            refused: false,
        },
        Case {
            label: "no configuration at all",
            configuration: None,
            refused: false,
        },
    ]
}

/// The source-map status and bucket a configuration names, as `(Status, S3Uri)` with an
/// absent member read as JSON null — the form both the request and the reported monitor
/// are compared in.
fn source_maps(configuration: Option<&Value>) -> (Value, Value) {
    match configuration {
        Some(configuration) => (
            configuration["JavaScriptSourceMaps"]["Status"].clone(),
            configuration["JavaScriptSourceMaps"]["S3Uri"].clone(),
        ),
        None => (Value::Null, Value::Null),
    }
}

/// Assert `reply` is the modelled refusal, stated in the documented words.
fn assert_refused(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        VALIDATION_CODE,
        "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    let stated = message(&body).unwrap_or_default();
    assert_eq!(
        stated, REFUSAL_MESSAGE,
        "case {case:?}: expected the message {REFUSAL_MESSAGE:?}, got {stated:?}"
    );
}

/// A monitor name for row `index`, inside `AppMonitorName`'s modelled pattern.
fn monitor_name(index: usize) -> String {
    format!("maps-monitor-{index}")
}

/// A create body for `name` carrying `configuration` when the row names one.
fn create_body(name: &str, configuration: Option<&Value>) -> Value {
    let mut body = json!({"Name": name, "Domain": "example.com"});
    if let Some(configuration) = configuration {
        body["DeobfuscationConfiguration"] = configuration.clone();
    }
    body
}

/// `GetAppMonitor` for `name`, raw, so a case can assert on a monitor that was never
/// created.
async fn get_raw(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/appmonitor/{name}"), None).await
}

#[tokio::test]
async fn create_refuses_source_maps_enabled_without_a_bucket() {
    let (router, _engine) = emulator();

    for (index, case) in cases().into_iter().enumerate() {
        let name = monitor_name(index);
        let label = case.label;
        let reply = call(
            &router,
            "POST",
            "/appmonitor",
            Some(create_body(&name, case.configuration.as_ref())),
        )
        .await;

        if case.refused {
            assert_refused(label, &reply);
            // Refused BEFORE anything is written: the name the request named is still
            // free, so there is no monitor to read back.
            let after = get_raw(&router, &name).await;
            assert_eq!(
                after.status,
                404,
                "case {label:?}: a refused create must write nothing, but GetAppMonitor \
                 answered {} {:?}",
                after.status,
                String::from_utf8_lossy(&after.body)
            );
            continue;
        }

        assert_eq!(
            reply.status,
            200,
            "case {label:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let monitor = get_app_monitor(&router, label, &name).await;
        let (want_status, want_uri) = source_maps(case.configuration.as_ref());
        let (got_status, got_uri) = source_maps(Some(&monitor["DeobfuscationConfiguration"]));
        // A request that named no configuration at all gets the documented default:
        // deobfuscation off, no bucket.
        let want_status = if want_status.is_null() && case.configuration.is_none() {
            json!("DISABLED")
        } else {
            want_status
        };
        assert_eq!(
            got_status, want_status,
            "case {label:?}: expected JavaScriptSourceMaps Status {want_status}, got {got_status}"
        );
        assert_eq!(
            got_uri, want_uri,
            "case {label:?}: expected JavaScriptSourceMaps S3Uri {want_uri}, got {got_uri}"
        );
    }
}

#[tokio::test]
async fn update_refuses_source_maps_enabled_without_a_bucket_and_writes_nothing() {
    let (router, _engine) = emulator();
    let name = "maps-monitor";
    let seeded = json!({
        "JavaScriptSourceMaps": {"Status": "ENABLED", "S3Uri": "s3://maps-bucket/app"}
    });
    create(&router, "seed", create_body(name, Some(&seeded))).await;
    let after_create = get_app_monitor(&router, "seed", name).await;
    assert_eq!(
        after_create["LastModified"],
        Value::Null,
        "case \"seed\": a monitor nothing has updated yet reports no LastModified, got {}",
        after_create["LastModified"]
    );

    // The configuration the monitor is holding as each row is applied: a refused update
    // leaves it exactly as it was, an accepted one replaces it.
    let mut held = seeded.clone();
    for case in cases() {
        let label = case.label;
        let Some(configuration) = case.configuration.as_ref() else {
            // `UpdateAppMonitor` has no "name no configuration" row to apply: an omitted
            // member keeps the stored one, which the lifecycle suite already pins.
            continue;
        };
        let before = get_app_monitor(&router, label, name).await;
        let reply = call(
            &router,
            "PATCH",
            &format!("/appmonitor/{name}"),
            Some(json!({"DeobfuscationConfiguration": configuration})),
        )
        .await;

        if case.refused {
            assert_refused(label, &reply);
        } else {
            assert_eq!(
                reply.status,
                200,
                "case {label:?}: expected UpdateAppMonitor to answer 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            held = configuration.clone();
        }

        let monitor = get_app_monitor(&router, label, name).await;
        let (want_status, want_uri) = source_maps(Some(&held));
        let (got_status, got_uri) = source_maps(Some(&monitor["DeobfuscationConfiguration"]));
        assert_eq!(
            got_status, want_status,
            "case {label:?}: expected JavaScriptSourceMaps Status {want_status}, got {got_status}"
        );
        assert_eq!(
            got_uri, want_uri,
            "case {label:?}: expected JavaScriptSourceMaps S3Uri {want_uri}, got {got_uri}"
        );
        if case.refused {
            // The refusal is taken before the record is touched, so the timestamp the
            // handler moves on a change has not moved either.
            assert_eq!(
                monitor["LastModified"], before["LastModified"],
                "case {label:?}: a refused update must not move LastModified, got {} where it \
                 held {}",
                monitor["LastModified"], before["LastModified"]
            );
        }
    }
}
