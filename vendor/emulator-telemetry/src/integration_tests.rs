// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! End-to-end integration tests: a real [`TelemetryClient`] speaking real
//! HTTP over TCP to [`crate::test_support::TestServer`], with real SQLite
//! state in a temp directory. No mocks anywhere.

use crate::client::{
    dispatch_generated_operation, init_global, record_generated_operation_with_handle,
    resolve_config_endpoint, CrashHook, ModelOperation, OperationFailure, OperationHandle,
    TelemetryClient, TelemetryOptions,
};
use crate::config::{Endpoint, ServerConfigCache};
use crate::metadata::sanitize_request_metadata;
use crate::outbox::Outbox;
use crate::policy::{
    config_cache_warning_line, invalid_switch_warning_line, state_init_warning_line,
    state_purge_warning_line, CLIENT_VERSION, CONFIG_RESPONSE_MAX_BODY_BYTES, DIMENSION_UNKNOWN,
    EMULATOR_CONFIG_FILE_NAME, INSTALLATION_ID_FILE, NOTICE_LOG_TARGET, OUTBOX_FILE,
    RECORD_QUEUE_BOUND, SERVER_CONFIG_CACHE_FILE, SQLITE_BUSY_TIMEOUT, STATE_OPEN_RETRY_PAUSE,
    SWITCH_OFF, SWITCH_ON, TELEMETRY_SWITCH_ENV,
};
use crate::test_support::{refused_base_url, FuzzRng, TestServer};
use crate::types::{
    ErrorType, InstallationId, Namespace, OperationName, Outcome, ProductVersion, ProjectId,
    Region, RunMode, StateArtifact, StatePurgeError, TelemetryFeatureFlags,
};
use crate::wire::epoch_seconds_now;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);
const TEST_PROJECT_ID: &str = "proj-0123456789abcdef0123456789abcdef";

/// The panic hook and its reporter slot are process-global; the tests that
/// install, retire, or fire it run one at a time so none observes another's
/// reporter.
fn panic_hook_serial() -> MutexGuard<'static, ()> {
    static PANIC_HOOK: Mutex<()> = Mutex::new(());
    PANIC_HOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn default_config_json() -> String {
    format!(
        r#"{{
            "enabled": true,
            "flushIntervalSeconds": 300,
            "maxRowsPerBatch": 1000,
            "maxBodyBytes": 262144,
            "samplingRate": 1.0,
            "minSupportedClientVersion": "{CLIENT_VERSION}",
            "configTtlSeconds": 3600
        }}"#
    )
}

/// Opt in via the config-file flag (never the process-global env var, which
/// would race across parallel tests). The emulator config file sits inside
/// the test's state dir; `client_for_project` points the client at it.
fn opt_in(state_dir: &Path) {
    std::fs::create_dir_all(state_dir).unwrap();
    std::fs::write(
        state_dir.join(EMULATOR_CONFIG_FILE_NAME),
        "[telemetry]\nenabled = true\n",
    )
    .unwrap();
}

fn client_for(server_base: &str, state_dir: &Path) -> TelemetryClient {
    client_for_project(server_base, state_dir, TEST_PROJECT_ID)
}

fn client_for_project(server_base: &str, state_dir: &Path, project_id: &str) -> TelemetryClient {
    let mut options = TelemetryOptions::new(
        ProjectId::new(project_id).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(server_base).unwrap(),
    );
    options.feature_flags = TelemetryFeatureFlags {
        persistence: true,
        multi_region: false,
        auth_emulation: true,
        strict_mode: false,
        request_logging: true,
        fidelity_gating: true,
    };
    options.state_dir = Some(state_dir.to_path_buf());
    options.set_config_file_override(state_dir.join(EMULATOR_CONFIG_FILE_NAME));
    options.set_retry_backoff_override(Duration::ZERO);
    TelemetryClient::start(options)
}

fn handle(namespace: &str, operation: &str) -> OperationHandle {
    OperationHandle::from_dimensions(
        Namespace::new(namespace).unwrap(),
        OperationName::new(operation).unwrap(),
    )
}

fn record_sample_op(client: &TelemetryClient, operation: &str, outcome: Outcome) {
    client.record_operation(
        &handle("sqs", operation),
        outcome,
        match outcome {
            Outcome::Success => None,
            _ => Some(ErrorType::Throttling),
        },
        Duration::from_millis(42),
    );
}

#[test]
fn delivers_the_exact_ndjson_shape_over_real_http() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.is_active());
    client.mark_ready();
    {
        let _first = client.begin_request();
        let _second = client.begin_request();
    }
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::USER_AGENT,
        "aws-sdk-rust/1.82.0 rust/1.85.1 customer@example.com /private/key"
            .parse()
            .unwrap(),
    );
    client.record_operation_with_metadata(
        &handle("sqs", "SendMessage"),
        Outcome::Success,
        None,
        Duration::from_millis(42),
        sanitize_request_metadata(&headers, Region::new("us-west-2").ok()),
    );
    record_sample_op(&client, "SendMessage", Outcome::Success);
    record_sample_op(&client, "ReceiveMessage", Outcome::ClientError);
    let before = epoch_seconds_now();
    assert!(client.flush_blocking(WAIT));

    let posts: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path == "/v1/events")
        .collect();
    assert_eq!(posts.len(), 1);
    assert_eq!(
        posts[0].header("content-type"),
        Some("application/x-ndjson")
    );

    let body = posts[0].body_utf8();
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        lines.len(),
        3,
        "batch envelope + two aggregated metric rows"
    );

    let session = &lines[0];
    assert_eq!(session["recordType"], "batch");
    assert_eq!(session["schemaVersion"], "1");
    assert_eq!(session["projectId"], TEST_PROJECT_ID);
    assert_eq!(session["flushSequence"], 1);
    assert_eq!(session["isFinal"], false);
    assert_eq!(session["productVersion"], "1.0");
    assert_eq!(session["firstRun"], true);
    assert_eq!(session["environment"]["sdkName"], "aws-sdk-rust");
    assert_eq!(session["environment"]["sdkVersion"], "1.82.0");
    assert_eq!(session["environment"]["languageVersion"], "1.85.1");
    assert_eq!(session["environment"]["region"], "us-west-2");
    assert_eq!(session["environment"]["runMode"], "standalone");
    assert!(session["environment"]["containerRuntime"].is_string());
    assert_eq!(
        session["environment"]["featureFlags"],
        serde_json::json!({
            "persistence": true,
            "multiRegion": false,
            "authEmulation": true,
            "strictMode": false,
            "requestLogging": true,
            "fidelityGating": true,
        })
    );
    assert_eq!(session["performance"]["peakConcurrency"], 2);
    assert!(session["performance"]["coldStartTime"].is_u64());
    assert_eq!(session["performance"]["startupOutcome"], "success");
    assert!(session["performance"]["memoryUsage"].is_string());
    let sent_at = session["sentAt"].as_str().unwrap();
    let parsed =
        time::OffsetDateTime::parse(sent_at, &time::format_description::well_known::Rfc3339)
            .unwrap();
    assert!(
        (before..=epoch_seconds_now()).contains(&(parsed.unix_timestamp() as u64)),
        "sentAt is the whole-second RFC3339 send time"
    );
    assert!(sent_at.ends_with('Z') && !sent_at.contains('.'));
    assert!(session["sessionDurationMs"].as_u64().is_some());
    let serialized_session = serde_json::to_string(session).unwrap();
    for forbidden in [
        "AKIAEXAMPLE",
        "Signature",
        "customer@example.com",
        "/private/key",
    ] {
        assert!(!serialized_session.contains(forbidden));
    }
    assert_eq!(server.config_requests()[0].path, "/v1/config");
    uuid::Uuid::parse_str(session["installationId"].as_str().unwrap()).unwrap();
    uuid::Uuid::parse_str(session["sessionId"].as_str().unwrap()).unwrap();
    uuid::Uuid::parse_str(session["batchId"].as_str().unwrap()).unwrap();
    assert!(
        session.get("crash").is_none(),
        "no crash on a clean session"
    );

    let send: &serde_json::Value = lines[1..]
        .iter()
        .find(|row| row["operation"] == "SendMessage")
        .expect("aggregated SendMessage row");
    assert_eq!(send["recordType"], "metric");
    assert_eq!(send["type"], "operation");
    assert_eq!(send["namespace"], "sqs");
    assert_eq!(send["count"], 2, "two observations aggregate into one row");
    assert_eq!(send["outcome"], "success");
    assert!(send.get("errorType").is_none());
    assert_eq!(send["latency"]["min"], 42);
    assert_eq!(send["latency"]["max"], 42);
    assert_eq!(send["latency"]["sumMillis"], 84);

    let receive: &serde_json::Value = lines[1..]
        .iter()
        .find(|row| row["operation"] == "ReceiveMessage")
        .expect("aggregated ReceiveMessage row");
    assert_eq!(receive["outcome"], "client_error");
    assert_eq!(receive["errorType"], "Throttling");

    client.shutdown(WAIT);
    let final_posts = server.ingest_bodies();
    let last = final_posts.last().unwrap();
    let final_session: serde_json::Value =
        serde_json::from_str(last.lines().next().unwrap()).unwrap();
    assert_eq!(final_session["isFinal"], true, "shutdown flush is final");
    assert_eq!(final_session["flushSequence"], 2);
}

#[test]
fn injected_ingest_endpoint_is_used_end_to_end() {
    let config_server = TestServer::start(&default_config_json());
    let ingest_server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&config_server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.ingest_endpoint =
        Some(Endpoint::parse_ingest(&format!("{}/v1/events", ingest_server.base_url)).unwrap());
    let client = TelemetryClient::start(options);
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    assert!(config_server.ingest_bodies().is_empty());
    assert_eq!(ingest_server.ingest_bodies().len(), 2);
}

#[test]
fn config_cache_and_etag_state_are_scoped_by_project() {
    const OTHER_PROJECT_ID: &str = "proj-fedcba9876543210fedcba9876543210";
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let first = client_for_project(&server.base_url, dir.path(), TEST_PROJECT_ID);
    assert!(first.flush_blocking(WAIT));
    first.shutdown(WAIT);

    let second = client_for_project(&server.base_url, dir.path(), OTHER_PROJECT_ID);
    assert!(second.flush_blocking(WAIT));
    second.shutdown(WAIT);

    assert_eq!(
        server.config_requests().len(),
        2,
        "a second project never reuses the first project's cached config"
    );
    let cache: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(SERVER_CONFIG_CACHE_FILE)).unwrap(),
    )
    .unwrap();
    let mut keys: Vec<&str> = cache
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, [TEST_PROJECT_ID, OTHER_PROJECT_ID]);
}

#[test]
fn honors_etag_with_if_none_match_and_304() {
    let mut config = default_config_json();
    config = config.replace("\"configTtlSeconds\": 3600", "\"configTtlSeconds\": 2");
    let server = TestServer::start(&config);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    assert_eq!(server.config_requests().len(), 1);
    assert_eq!(
        server.config_requests()[0].header("if-none-match"),
        None,
        "nothing cached on first fetch"
    );

    std::thread::sleep(Duration::from_millis(2100));
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));

    let config_requests = server.config_requests();
    assert_eq!(config_requests.len(), 2, "TTL lapse forces a revalidation");
    assert_eq!(
        config_requests[1].header("if-none-match"),
        Some("\"test-etag-1\""),
        "revalidation carries the cached ETag"
    );
    assert_eq!(
        server.ingest_bodies().len(),
        2,
        "the 304 kept the cached config live and ingest kept flowing"
    );
    client.shutdown(WAIT);
}

#[test]
fn shutdown_uses_cached_config_without_revalidation() {
    let config =
        default_config_json().replace("\"configTtlSeconds\": 3600", "\"configTtlSeconds\": 1");
    let server = TestServer::start(&config);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "BeforeShutdown", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    // Under parallel-suite load more than one TTL (1s) can lapse before the
    // first flush completes, so the pre-shutdown fetch count is not exactly 1.
    // The invariant under test is the DELTA: shutdown adds no config request.
    let config_requests_before_shutdown = server.config_requests().len();
    assert!(config_requests_before_shutdown >= 1);
    std::thread::sleep(Duration::from_millis(1100));

    record_sample_op(&client, "FinalOperation", Outcome::Success);
    client.shutdown(WAIT);

    assert_eq!(
        server.config_requests().len(),
        config_requests_before_shutdown,
        "bounded shutdown must not start a config request after its deadline begins"
    );
    assert!(
        server
            .ingest_bodies()
            .iter()
            .any(|body| body.contains("FinalOperation")),
        "the cached config still permits the final flush"
    );
}

#[test]
fn shutdown_without_a_cache_keeps_the_session_decision_not_the_defaults() {
    // Two rows per batch is the envelope plus one metric, so the server's
    // decision shows as one batch per operation; the shipped default does not.
    let config =
        default_config_json().replace("\"maxRowsPerBatch\": 1000", "\"maxRowsPerBatch\": 2");
    let server = TestServer::start(&config);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.flush_blocking(WAIT));
    std::fs::remove_file(dir.path().join(SERVER_CONFIG_CACHE_FILE)).unwrap();

    record_sample_op(&client, "FirstOperation", Outcome::Success);
    record_sample_op(&client, "SecondOperation", Outcome::Success);
    client.shutdown(WAIT);

    let final_bodies: Vec<String> = server
        .ingest_bodies()
        .into_iter()
        .filter(|body| body.contains("Operation\""))
        .collect();
    assert_eq!(
        final_bodies.len(),
        2,
        "the final flush must batch on the session's server decision, not the shipped default"
    );
}

#[test]
fn oversized_config_response_is_rejected_and_negatively_cached() {
    let oversized = format!(
        "{{\"enabled\":true,\"padding\":\"{}\"}}",
        "x".repeat(CONFIG_RESPONSE_MAX_BODY_BYTES)
    );
    let server = TestServer::start(&oversized);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "FallbackOperation", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    assert_eq!(
        server.config_requests().len(),
        1,
        "a rejected config response is not refetched on every flush"
    );
    client.shutdown(WAIT);
    assert_eq!(
        server.ingest_bodies().len(),
        0,
        "with no cached config a rejected response is no decision: nothing is sent"
    );
    assert_eq!(
        persisted_installation_id(dir.path()),
        None,
        "no installation id is written before the server allows collection"
    );
}

/// The final flush never falls back to the shipped defaults: once the
/// session's own decision has expired and no cached config is left, the
/// shutdown sends nothing.
#[test]
fn shutdown_without_a_cache_or_a_current_session_decision_sends_nothing() {
    let config =
        default_config_json().replace("\"configTtlSeconds\": 3600", "\"configTtlSeconds\": 1");
    let server = TestServer::start(&config);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.flush_blocking(WAIT));
    std::fs::remove_file(dir.path().join(SERVER_CONFIG_CACHE_FILE)).unwrap();
    record_sample_op(&client, "FinalOperation", Outcome::Success);
    std::thread::sleep(Duration::from_millis(2100));
    client.shutdown(WAIT);

    assert!(
        !server
            .ingest_bodies()
            .iter()
            .any(|body| body.contains("FinalOperation")),
        "the final flush sends nothing on terms the server did not set"
    );
}

#[test]
fn splits_batches_client_side_at_the_byte_cap() {
    let config =
        default_config_json().replace("\"maxRowsPerBatch\": 1000", "\"maxRowsPerBatch\": 100000");
    let server = TestServer::start(&config);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    for i in 0..2500 {
        client.record_operation(
            &handle(
                "dynamodb",
                &format!("Operation{i:05}Wide{}", "x".repeat(38)),
            ),
            Outcome::Success,
            None,
            Duration::from_millis(7),
        );
    }
    assert!(client.flush_blocking(WAIT));

    let bodies = server.ingest_bodies();
    assert!(
        bodies.len() >= 2,
        "2500 wide rows cannot fit one 256 KiB batch (got {})",
        bodies.len()
    );
    let mut total_metric_rows = 0usize;
    let mut batch_ids = std::collections::BTreeSet::new();
    for body in &bodies {
        assert!(
            body.len() <= 262_144,
            "every batch respects maxBodyBytes (got {})",
            body.len()
        );
        let first: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        assert_eq!(first["recordType"], "batch");
        batch_ids.insert(first["batchId"].as_str().unwrap().to_string());
        total_metric_rows += body.lines().count() - 1;
    }
    assert_eq!(total_metric_rows, 2500, "no row lost to splitting");
    assert_eq!(batch_ids.len(), bodies.len(), "fresh batch id per batch");
    client.shutdown(WAIT);
}

#[test]
fn splits_batches_on_the_servers_400_response() {
    let server = TestServer::start(&default_config_json());
    server
        .behavior
        .ingest_reject_over_bytes
        .store(2048, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    for i in 0..24 {
        record_sample_op(&client, &format!("Operation{i:03}"), Outcome::Success);
    }
    // The first flush gets a 400 and splits; each following flush retries the
    // (immediately due) halves until every piece fits under the cap.
    for _ in 0..8 {
        assert!(client.flush_blocking(WAIT));
    }

    let mut delivered = std::collections::BTreeSet::new();
    for body in server.ingest_bodies() {
        if body.len() > 2048 {
            continue; // the rejected originals
        }
        for line in body.lines().skip(1) {
            let row: serde_json::Value = serde_json::from_str(line).unwrap();
            delivered.insert(row["operation"].as_str().unwrap().to_string());
        }
    }
    assert_eq!(delivered.len(), 24, "every metric row survived the splits");
    client.shutdown(WAIT);
}

#[test]
fn outbox_survives_process_restart_and_drains_later() {
    let server = TestServer::start(&default_config_json());
    server
        .behavior
        .ingest_fail_all
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let first = client_for(&server.base_url, dir.path());
    record_sample_op(&first, "PersistedOperation", Outcome::Success);
    assert!(first.flush_blocking(WAIT));
    first.shutdown(WAIT);

    {
        let outbox = Outbox::open(dir.path()).unwrap();
        assert!(
            outbox.pending_count().unwrap() >= 1,
            "failed batches remain durable across shutdown"
        );
    }

    server
        .behavior
        .ingest_fail_all
        .store(false, Ordering::SeqCst);
    let second = client_for(&server.base_url, dir.path());
    assert!(second.flush_blocking(WAIT));
    second.shutdown(WAIT);

    let persisted_bodies: Vec<_> = server
        .ingest_bodies()
        .into_iter()
        .filter(|body| body.contains("PersistedOperation"))
        .collect();
    assert!(
        persisted_bodies.len() >= 2,
        "the failed request was retried after process restart"
    );
    let batch_ids: std::collections::BTreeSet<_> = persisted_bodies
        .iter()
        .map(|body| {
            let session: serde_json::Value =
                serde_json::from_str(body.lines().next().unwrap()).unwrap();
            session["batchId"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(batch_ids.len(), 1, "a 5xx retry keeps the dedup batch id");

    let outbox = Outbox::open(dir.path()).unwrap();
    assert_eq!(outbox.pending_count().unwrap(), 0, "outbox fully drained");
}

#[test]
fn throttled_batch_is_retried_with_the_same_dedup_id() {
    let server = TestServer::start(&default_config_json());
    server
        .behavior
        .ingest_throttle_all
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "ThrottledOperation", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    server
        .behavior
        .ingest_throttle_all
        .store(false, Ordering::SeqCst);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    let bodies: Vec<_> = server
        .ingest_bodies()
        .into_iter()
        .filter(|body| body.contains("ThrottledOperation"))
        .collect();
    assert!(bodies.len() >= 2, "the 429 response schedules a retry");
    let batch_ids: std::collections::BTreeSet<_> = bodies
        .iter()
        .map(|body| {
            let session: serde_json::Value =
                serde_json::from_str(body.lines().next().unwrap()).unwrap();
            session["batchId"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(batch_ids.len(), 1, "a 429 retry keeps the dedup batch id");
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        0
    );
}

#[test]
fn installation_id_is_stable_across_sessions() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let first = client_for(&server.base_url, dir.path());
    assert!(first.flush_blocking(WAIT));
    first.shutdown(WAIT);
    let second = client_for(&server.base_url, dir.path());
    assert!(second.flush_blocking(WAIT));
    second.shutdown(WAIT);

    let ids: std::collections::BTreeSet<String> = server
        .ingest_bodies()
        .iter()
        .map(|body| {
            let session: serde_json::Value =
                serde_json::from_str(body.lines().next().unwrap()).unwrap();
            session["installationId"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(ids.len(), 1, "one stable installation id across sessions");

    let sessions: std::collections::BTreeSet<String> = server
        .ingest_bodies()
        .iter()
        .map(|body| {
            let session: serde_json::Value =
                serde_json::from_str(body.lines().next().unwrap()).unwrap();
            session["sessionId"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(sessions.len(), 2, "fresh session id per process");

    let second_session: serde_json::Value = serde_json::from_str(
        server
            .ingest_bodies()
            .last()
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        second_session["firstRun"], false,
        "only the very first session is firstRun"
    );
}

#[test]
fn every_silent_source_means_telemetry_is_on_by_default() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    let client = client_for(&server.base_url, dir.path());
    assert!(client.is_active());
    assert!(
        client
            .startup_notice()
            .contains("enabled (on by default; opt out any time)"),
        "the notice announces the opt-out default: {:?}",
        client.startup_notice()
    );

    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);
    assert!(
        !server.ingest_bodies().is_empty(),
        "a configless client sends once the server config allows it"
    );
}

#[test]
fn explicit_file_opt_out_sends_nothing() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(
        dir.path().join(EMULATOR_CONFIG_FILE_NAME),
        "[telemetry]\nenabled = false\n",
    )
    .unwrap();

    let client = client_for(&server.base_url, dir.path());
    assert!(!client.is_active());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    client.shutdown(Duration::from_secs(1));
    assert!(server.requests().is_empty());
}

#[test]
fn an_embedding_api_off_wins_over_a_file_opt_in() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.embedding_opt_out = true;
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    let client = TelemetryClient::start(options);

    assert!(!client.is_active(), "any off wins: file on, embedding off");
    let notice = client.startup_notice();
    assert!(notice.contains("disabled"));
    assert!(
        notice.contains("via the embedding configuration"),
        "the notice names the deciding source: {notice:?}"
    );
    record_sample_op(&client, "SendMessage", Outcome::Success);
    client.shutdown(Duration::from_secs(1));
    assert!(
        server.requests().is_empty(),
        "an embedding off performs zero HTTP requests"
    );
}

#[test]
fn the_embedding_api_cannot_force_telemetry_on() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.embedding_opt_out = false;
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.set_retry_backoff_override(Duration::ZERO);
    let client = TelemetryClient::start(options);

    assert_eq!(
        client.is_active(),
        emulator_telemetry_default_enabled(),
        "a non-opted-out embedding is silent: with every user source silent the \
         compiled default decides — the embedding API is disable-only and has \
         no way to force telemetry on"
    );
    let notice = client.startup_notice();
    assert!(
        notice.contains("(on by default"),
        "the notice names the default as the deciding source: {notice:?}"
    );
    record_sample_op(&client, "SendMessage", Outcome::Success);
    client.shutdown(Duration::from_secs(1));
    if !emulator_telemetry_default_enabled() {
        assert!(
            server.requests().is_empty(),
            "a default-off client with a silent embedding performs zero HTTP requests"
        );
    }
}

fn emulator_telemetry_default_enabled() -> bool {
    crate::policy::TELEMETRY_DEFAULT_ENABLED
}

/// Every file in `state_dir` that the purge is responsible for: the
/// installation id with its quarantine/publish siblings and the outbox with
/// its WAL/SHM journals. The user's config file and the server-config cache
/// are deliberately not telemetry's to delete.
fn persisted_artifacts(state_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(INSTALLATION_ID_FILE) || name.starts_with(OUTBOX_FILE))
        .collect();
    names.sort();
    names
}

fn assert_no_persisted_state(state_dir: &Path, context: &str) {
    assert_eq!(
        persisted_artifacts(state_dir),
        Vec::<String>::new(),
        "{context}: the installation id and the outbox (with every sibling) must be deleted"
    );
}

fn assert_only_the_installation_id_persists(state_dir: &Path, context: &str) {
    assert_eq!(
        persisted_artifacts(state_dir),
        vec![INSTALLATION_ID_FILE.to_string()],
        "{context}: only the installation id is kept, so the sampling decision stays stable"
    );
}

fn assert_denied_for_the_session(client: &TelemetryClient, context: &str) {
    assert!(
        !client.recording_active(),
        "{context}: recording must be off for the rest of the session"
    );
    assert!(
        !client.is_active(),
        "{context}: the client reports itself off"
    );
    assert_eq!(
        client.state_purge_error(),
        None,
        "{context}: the purge must succeed"
    );
}

#[test]
fn server_kill_switch_stops_sending() {
    let config = default_config_json().replace("\"enabled\": true", "\"enabled\": false");
    let server = TestServer::start(&config);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    assert_denied_for_the_session(&client, "server kill switch");
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    assert!(
        !server.config_requests().is_empty(),
        "the client consulted the kill switch"
    );
    assert!(
        server.ingest_bodies().is_empty(),
        "a server-disabled client never posts events"
    );
    assert_no_persisted_state(dir.path(), "server kill switch");
}

fn persist_installation_id(state_dir: &Path) -> InstallationId {
    let id = InstallationId::random();
    std::fs::write(
        state_dir.join(INSTALLATION_ID_FILE),
        format!("{}\n", id.as_str()),
    )
    .unwrap();
    id
}

fn persisted_installation_id(state_dir: &Path) -> Option<InstallationId> {
    std::fs::read_to_string(state_dir.join(INSTALLATION_ID_FILE))
        .ok()
        .map(|content| InstallationId::new(content.trim()).unwrap())
}

/// A fresh installation the server does not sample never writes an id (the
/// sampling purge keeps ids, so one written early would survive); a returning
/// one keeps the id its sampling decision is derived from.
#[test]
fn sampled_out_installation_sends_no_events() {
    let config = default_config_json().replace("\"samplingRate\": 1.0", "\"samplingRate\": 0.0");
    for returning in [false, true] {
        let context = format!("unsampled installation (returning: {returning})");
        let server = TestServer::start(&config);
        let dir = tempfile::tempdir().unwrap();
        opt_in(dir.path());
        let seeded = returning.then(|| persist_installation_id(dir.path()));

        let client = client_for(&server.base_url, dir.path());
        record_sample_op(&client, "SendMessage", Outcome::Success);
        assert!(client.flush_blocking(WAIT));
        assert_denied_for_the_session(&client, &context);
        client.shutdown(WAIT);
        assert!(server.ingest_bodies().is_empty());
        match seeded {
            Some(id) => {
                assert_only_the_installation_id_persists(dir.path(), &context);
                assert_eq!(persisted_installation_id(dir.path()), Some(id));
            }
            None => assert_no_persisted_state(dir.path(), &context),
        }
    }
}

/// While the server decision is pending the id exists only in memory, so a
/// remote disable arriving then finds no identifier ever written.
#[test]
fn a_remote_disable_never_writes_an_installation_id() {
    let config = default_config_json().replace("\"enabled\": true", "\"enabled\": false");
    let server = TestServer::start(&config);
    let release = server.hold_config_responses();
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    server.wait_for_first_request();
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert_eq!(
        persisted_installation_id(dir.path()),
        None,
        "no installation id is written before the server decides"
    );

    drop(release);
    assert!(client.flush_blocking(WAIT));
    assert_denied_for_the_session(&client, "remote disable");
    client.shutdown(WAIT);
    assert!(server.ingest_bodies().is_empty());
    assert_no_persisted_state(dir.path(), "remote disable");
}

#[test]
fn a_fresh_installation_id_is_written_once_the_server_allows_collection() {
    let server = TestServer::start(&default_config_json());
    let release = server.hold_config_responses();
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    server.wait_for_first_request();
    assert_eq!(
        persisted_installation_id(dir.path()),
        None,
        "no installation id is written before the server decides"
    );

    drop(release);
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    let persisted =
        persisted_installation_id(dir.path()).expect("an allowed session persists its id");
    let bodies = server.ingest_bodies();
    assert!(!bodies.is_empty(), "the allowed session posts");
    for body in &bodies {
        let session: serde_json::Value =
            serde_json::from_str(body.lines().next().unwrap()).unwrap();
        assert_eq!(session["installationId"], persisted.as_str());
        assert_eq!(session["firstRun"], true);
    }
}

/// A concurrent starter that persists its id while this session's decision
/// is pending wins: its id is adopted and re-sampled, and this session does
/// not claim `firstRun`.
#[test]
fn an_installation_id_persisted_while_the_decision_is_pending_is_adopted() {
    let unsampled_winner = std::iter::repeat_with(InstallationId::random)
        .find(|id| !crate::config::is_sampled(id, 0.5))
        .unwrap();
    for (sampling_rate, winner) in [("1.0", InstallationId::random()), ("0.5", unsampled_winner)] {
        let context = format!("adopted winner at sampling rate {sampling_rate}");
        let config = default_config_json().replace(
            "\"samplingRate\": 1.0",
            &format!("\"samplingRate\": {sampling_rate}"),
        );
        let server = TestServer::start(&config);
        let release = server.hold_config_responses();
        let dir = tempfile::tempdir().unwrap();
        opt_in(dir.path());

        let client = client_for(&server.base_url, dir.path());
        server.wait_for_first_request();
        std::fs::write(
            dir.path().join(INSTALLATION_ID_FILE),
            format!("{}\n", winner.as_str()),
        )
        .unwrap();
        drop(release);
        record_sample_op(&client, "SendMessage", Outcome::Success);
        assert!(client.flush_blocking(WAIT));
        client.shutdown(WAIT);

        assert_eq!(
            persisted_installation_id(dir.path()),
            Some(winner.clone()),
            "{context}"
        );
        let bodies = server.ingest_bodies();
        if crate::config::is_sampled(&winner, sampling_rate.parse().unwrap()) {
            assert!(!bodies.is_empty(), "{context}: the adopted id is sampled");
            for body in &bodies {
                let session: serde_json::Value =
                    serde_json::from_str(body.lines().next().unwrap()).unwrap();
                assert_eq!(session["installationId"], winner.as_str(), "{context}");
                assert_eq!(session["firstRun"], false, "{context}");
            }
        } else {
            assert!(
                bodies.is_empty(),
                "{context}: the adopted id is not sampled"
            );
            assert_only_the_installation_id_persists(dir.path(), &context);
        }
    }
}

/// An id that cannot be written after the server allows collection closes
/// the session without sending, creating an id, or deleting queued data.
#[test]
fn an_unwritable_installation_id_stops_the_session_and_keeps_the_outbox() {
    let server = TestServer::start(&default_config_json());
    let release = server.hold_config_responses();
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    server.wait_for_first_request();
    std::fs::create_dir(dir.path().join(INSTALLATION_ID_FILE)).unwrap();
    record_sample_op(&client, "SendMessage", Outcome::Success);
    drop(release);
    assert!(client.flush_blocking(WAIT));
    assert!(!client.recording_active(), "the session stops recording");
    client.shutdown(WAIT);

    assert!(server.ingest_bodies().is_empty(), "nothing is sent");
    assert!(
        dir.path().join(INSTALLATION_ID_FILE).is_dir(),
        "no id is created"
    );
    assert!(
        persisted_artifacts(dir.path())
            .iter()
            .any(|name| name == OUTBOX_FILE),
        "the outbox is kept: {:?}",
        persisted_artifacts(dir.path())
    );
}

#[test]
fn client_below_minimum_version_disables_itself() {
    let config = default_config_json().replace(
        &format!("\"minSupportedClientVersion\": \"{CLIENT_VERSION}\""),
        "\"minSupportedClientVersion\": \"99.0.0\"",
    );
    let server = TestServer::start(&config);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    assert_denied_for_the_session(&client, "unsupported client version");
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    assert!(
        server.ingest_bodies().is_empty(),
        "below-minimum sends nothing"
    );
    assert_eq!(
        server.config_requests().len(),
        1,
        "after the version gate trips, the client goes fully quiet"
    );
    assert_no_persisted_state(dir.path(), "unsupported client version");
}

#[test]
fn remote_disable_purges_records_queued_before_the_decision() {
    let config =
        default_config_json().replace("\"configTtlSeconds\": 3600", "\"configTtlSeconds\": 1");
    let server = TestServer::start(&config);
    server
        .behavior
        .ingest_fail_all
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);
    assert!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap() > 0,
        "the failed delivery left a queued batch behind"
    );

    server
        .behavior
        .ingest_fail_all
        .store(false, Ordering::SeqCst);
    *server.behavior.config_json.lock().unwrap() =
        config.replace("\"enabled\": true", "\"enabled\": false");
    *server.behavior.etag.lock().unwrap() = "\"disabled\"".to_string();
    let ingest_posts_before = server.ingest_bodies().len();
    std::thread::sleep(Duration::from_millis(1100));
    let disabled = client_for(&server.base_url, dir.path());
    assert!(disabled.flush_blocking(WAIT));
    assert_denied_for_the_session(&disabled, "remote disable after queueing");
    disabled.shutdown(WAIT);

    assert_eq!(
        server.ingest_bodies().len(),
        ingest_posts_before,
        "records queued under an enabled config are never sent once the server disables collection"
    );
    assert_no_persisted_state(dir.path(), "remote disable after queueing");
}

#[test]
fn panic_hook_reports_crash_type_only() {
    let _serial = panic_hook_serial();
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(
        client.flush_blocking(WAIT),
        "the collection decision has landed"
    );
    client.install_panic_hook();

    const SECRET: &str = "SECRET_PANIC_MESSAGE_MUST_NOT_LEAK_7f3a";
    let panicker = std::thread::spawn(|| panic!("{SECRET}"));
    assert!(panicker.join().is_err());

    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    let bodies = server.ingest_bodies();
    let crash_sessions: Vec<serde_json::Value> = bodies
        .iter()
        .flat_map(|body| body.lines())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| row.get("crash").is_some())
        .collect();
    assert_eq!(crash_sessions.len(), 1, "exactly one crash record");
    let crash = &crash_sessions[0];
    assert_eq!(crash["crash"]["crashType"], "panic");
    assert_eq!(crash["isFinal"], false, "a crash is not a clean shutdown");
    assert_eq!(
        crash["crash"].as_object().unwrap().len(),
        1,
        "crash carries the type and NOTHING else — no location, no message"
    );
    for body in &bodies {
        assert!(
            !body.contains(SECRET),
            "the panic message must never reach the wire"
        );
        assert!(
            !body.contains("integration_tests.rs"),
            "the panic location must never reach the wire"
        );
    }
}

#[test]
fn startup_notice_is_a_single_line_naming_the_controls() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let active = client_for(&server.base_url, dir.path());
    let notice = active.startup_notice();
    assert!(!notice.contains('\n'), "one line: {notice:?}");
    assert!(notice.starts_with("This emulator collects usage and error telemetry: enabled"));
    assert!(notice.contains(&format!("{TELEMETRY_SWITCH_ENV}={SWITCH_OFF}")));
    assert!(
        notice.contains(
            &dir.path()
                .join(EMULATOR_CONFIG_FILE_NAME)
                .display()
                .to_string()
        ),
        "the notice prints the resolved config path: {notice:?}"
    );
    active.shutdown(WAIT);

    let off_dir = tempfile::tempdir().unwrap();
    let configless = client_for(&server.base_url, off_dir.path());
    let notice = configless.startup_notice();
    assert!(!notice.contains('\n'));
    assert!(notice.starts_with("This emulator collects usage and error telemetry: enabled"));
    configless.shutdown(WAIT);
}

/// First run offline: with no cached server config and no response, the
/// server has never allowed collection, so the shipped defaults never stand in
/// for it — no installation id is written and nothing is queued or sent.
#[test]
fn first_run_with_an_unreachable_config_endpoint_writes_and_queues_nothing() {
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&refused_base_url(), dir.path());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    assert!(
        !client.recording_active(),
        "no server decision yet means no collection"
    );
    assert!(client.config_cache_error().is_none());
    assert!(client.state_purge_error().is_none());
    client.shutdown(WAIT);

    assert_eq!(
        persisted_installation_id(dir.path()),
        None,
        "no installation id is written before the server allows collection"
    );
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        0,
        "nothing is queued before the server allows collection"
    );
}

/// A cached allow keeps deciding while the config endpoint is unreachable:
/// the offline session collects, queues and attempts delivery as before.
#[test]
fn a_cached_enabled_config_keeps_collecting_while_the_endpoint_is_unreachable() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let online = client_for(&server.base_url, dir.path());
    assert!(online.flush_blocking(WAIT));
    online.shutdown(WAIT);
    let installation_id = persisted_installation_id(dir.path()).expect("the server allowed it");
    // Expire the cached decision so the offline start must try the endpoint.
    ServerConfigCache::open(dir.path())
        .update(&ProjectId::new(TEST_PROJECT_ID).unwrap(), |entry| {
            entry.fetched.as_mut().unwrap().fetched_at = 0;
        })
        .unwrap();

    let client = client_for(&refused_base_url(), dir.path());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    assert!(
        client.recording_active(),
        "an unreachable endpoint never overrides the cached allow"
    );
    client.shutdown(WAIT);

    assert_eq!(persisted_installation_id(dir.path()), Some(installation_id));
    let outbox = Outbox::open(dir.path()).unwrap();
    let due = outbox.due_batches(u64::MAX, 100).unwrap();
    assert!(
        due.iter().any(|b| b.body.contains("SendMessage")),
        "the cached allow keeps building batches while /v1/config is unreachable"
    );
    assert!(
        due.iter().any(|b| b.attempts >= 1),
        "delivery was attempted and rescheduled for retry, not dropped"
    );
}

#[test]
fn startup_fetches_the_config_immediately() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    // No record, no flush: the config consultation must come from startup,
    // not from a flush (the server-directed interval is 300 s).
    let client = client_for(&server.base_url, dir.path());
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline && server.config_requests().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !server.config_requests().is_empty(),
        "the worker consults /v1/config at startup rather than waiting out the \
         first flush interval"
    );
    assert!(
        server.ingest_bodies().is_empty(),
        "nothing recorded, nothing sent"
    );
    client.shutdown(WAIT);
}

#[test]
fn startup_drains_the_outbox_without_any_flush() {
    let server = TestServer::start(&default_config_json());
    server
        .behavior
        .ingest_fail_all
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let first = client_for(&server.base_url, dir.path());
    record_sample_op(&first, "PersistedOperation", Outcome::Success);
    assert!(first.flush_blocking(WAIT));
    first.shutdown(WAIT);
    {
        let outbox = Outbox::open(dir.path()).unwrap();
        assert!(outbox.pending_count().unwrap() >= 1, "a batch is stranded");
    }

    server
        .behavior
        .ingest_fail_all
        .store(false, Ordering::SeqCst);
    // The second session records nothing and never flushes: startup alone
    // must deliver the stranded batch.
    let second = client_for(&server.base_url, dir.path());
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline
        && !server
            .ingest_bodies()
            .iter()
            .any(|b| b.contains("PersistedOperation"))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        server
            .ingest_bodies()
            .iter()
            .any(|b| b.contains("PersistedOperation")),
        "the worker drains persisted batches at startup, before any flush"
    );
    second.shutdown(WAIT);
}

#[test]
fn shutdown_stays_bounded_when_the_server_stalls() {
    // A real listener that accepts and never answers: the worker wedges on
    // its startup config fetch until reqwest's request timeout, well past
    // the shutdown budget below.
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept() {
            held.push(stream);
        }
    });

    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let client = client_for(&format!("http://{addr}"), dir.path());
    record_sample_op(&client, "SendMessage", Outcome::Success);

    let budget = Duration::from_secs(2);
    let started = std::time::Instant::now();
    client.shutdown(budget);
    let elapsed = started.elapsed();
    assert!(
        elapsed < budget + Duration::from_secs(2),
        "shutdown must abandon a wedged worker at its budget, never join it \
         unbounded (took {elapsed:?} against a {budget:?} budget)"
    );
}

struct SendMessageOp;
impl ModelOperation for SendMessageOp {
    const NAMESPACE: &'static str = "sqs";
    const OPERATION: &'static str = "SendMessage";
}

struct CreateQueueOp;
impl ModelOperation for CreateQueueOp {
    const NAMESPACE: &'static str = "sqs";
    const OPERATION: &'static str = "CreateQueue";
}

struct ExistingOp;
impl ModelOperation for ExistingOp {
    const NAMESPACE: &'static str = "sqs";
    const OPERATION: &'static str = "ExistingOperation";
}

struct ArnNamespacedOp;
impl ModelOperation for ArnNamespacedOp {
    const NAMESPACE: &'static str = "arn:aws:iam::123456789012:role/Admin";
    const OPERATION: &'static str = "customer@example.com";
}

#[test]
fn generated_operation_shim_records_protocol_outcomes_via_the_global_client() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let client = client_for(&server.base_url, dir.path());
    let global = init_global(client.clone());

    let send_message = OperationHandle::of::<SendMessageOp>();
    record_generated_operation_with_handle(&send_message, Duration::from_millis(12), None, None);
    record_generated_operation_with_handle(
        &send_message,
        Duration::from_millis(30),
        Some(OperationFailure::classify(400, "ThrottlingException")),
        None,
    );
    record_generated_operation_with_handle(
        &OperationHandle::of::<CreateQueueOp>(),
        Duration::from_millis(3),
        Some(OperationFailure::classify(501, "NotImplemented")),
        None,
    );
    record_generated_operation_with_handle(
        &OperationHandle::of::<ExistingOp>(),
        Duration::from_millis(9),
        Some(OperationFailure::classify(501, "InternalFailure")),
        None,
    );
    let folded = OperationHandle::of::<ArnNamespacedOp>();
    assert_eq!(
        (folded.namespace().as_str(), folded.operation().as_str()),
        (DIMENSION_UNKNOWN, DIMENSION_UNKNOWN),
        "identifier-shaped dimensions fold to the unknown token before anything is stored"
    );
    record_generated_operation_with_handle(&folded, Duration::from_millis(1), None, None);

    assert!(global.flush_blocking(WAIT));
    let bodies = server.ingest_bodies();
    let rows: Vec<serde_json::Value> = bodies
        .iter()
        .flat_map(|body| body.lines())
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|row: &serde_json::Value| row["recordType"] == "metric")
        .collect();

    let success = rows
        .iter()
        .find(|r| r["operation"] == "SendMessage" && r["outcome"] == "success")
        .expect("a success operation row");
    assert_eq!(success["type"], "operation");
    assert_eq!(success["namespace"], "sqs");
    assert_eq!(success["count"], 1);
    assert_eq!(success["latency"]["min"], 12);
    assert!(success.get("errorType").is_none());

    let failed = rows
        .iter()
        .find(|r| r["operation"] == "SendMessage" && r["outcome"] == "client_error")
        .expect("a folded modeled-failure row");
    assert_eq!(failed["type"], "operation");
    assert_eq!(
        failed["errorType"], "Throttling",
        "the raw shape name folds into the closed taxonomy"
    );
    assert_eq!(failed["latency"]["min"], 30);

    let gap = rows
        .iter()
        .find(|r| r["operation"] == "CreateQueue")
        .expect("a coverage-gap row");
    assert_eq!(gap["type"], "coverage_gap");
    assert_eq!(gap["outcome"], "not_emulated");
    assert!(gap.get("errorType").is_none());
    assert!(
        gap.get("latency").is_none(),
        "a coverage gap carries no latency"
    );

    let modeled_501 = rows
        .iter()
        .find(|r| r["operation"] == "ExistingOperation")
        .expect("a modeled 501 operation row");
    assert_eq!(modeled_501["type"], "operation");
    assert_eq!(modeled_501["outcome"], "server_error");
    assert_eq!(modeled_501["errorType"], "InternalError");
    assert_eq!(modeled_501["latency"]["min"], 9);

    let folded_row = rows
        .iter()
        .find(|r| r["namespace"] == DIMENSION_UNKNOWN)
        .expect("the folded operation is still counted, under the unknown token");
    assert_eq!(folded_row["operation"], DIMENSION_UNKNOWN);

    for body in &bodies {
        assert!(
            !body.contains("arn:aws") && !body.contains("example.com"),
            "an identifier-shaped dimension never reaches the wire"
        );
        assert!(
            !body.contains("ThrottlingException"),
            "the raw error shape name never reaches the wire"
        );
    }
    global.shutdown(WAIT);
}

#[test]
fn generated_operation_request_metadata_reaches_the_batch_without_the_raw_user_agent() {
    const RAW_USER_AGENT: &str =
        "aws-sdk-rust/1.82.0 os/linux lang/rust/1.85.1 md/claude-code customer@example.com";
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let client = client_for(&server.base_url, dir.path());

    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::USER_AGENT, RAW_USER_AGENT.parse().unwrap());
    dispatch_generated_operation(
        &client,
        &OperationHandle::of::<SendMessageOp>(),
        Duration::from_millis(5),
        None,
        Some(sanitize_request_metadata(
            &headers,
            Region::new("eu-west-1").ok(),
        )),
    );
    assert!(client.flush_blocking(WAIT));

    let bodies = server.ingest_bodies();
    let batch: serde_json::Value = serde_json::from_str(bodies[0].lines().next().unwrap()).unwrap();
    let environment = &batch["environment"];
    assert_eq!(environment["sdkName"], "aws-sdk-rust");
    assert_eq!(environment["sdkVersion"], "1.82.0");
    assert_eq!(environment["languageVersion"], "1.85.1");
    assert_eq!(environment["agentClient"], "claude-code");
    assert_eq!(environment["region"], "eu-west-1");
    for body in &bodies {
        for raw in [
            RAW_USER_AGENT,
            "customer@example.com",
            "md/claude-code",
            "os/linux",
        ] {
            assert!(!body.contains(raw), "{raw} reached the wire: {body}");
        }
    }
    client.shutdown(WAIT);
}

#[test]
fn unusable_state_storage_stops_collection_with_one_warning() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    std::fs::write(dir.path().join(INSTALLATION_ID_FILE), b"not-a-uuid\n").unwrap();
    std::fs::write(dir.path().join(OUTBOX_FILE), b"not a sqlite database").unwrap();
    let notices: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    let sink = notices.clone();
    options.set_notice_sink_override(Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    }));
    let client = TelemetryClient::start(options);
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(
        client.flush_blocking(WAIT),
        "a client whose state failed to open acknowledges flushes"
    );
    assert!(!client.is_active(), "a corrupt outbox stops collection");
    assert!(!client.recording_active());
    client.shutdown(Duration::from_secs(1));

    let lines = notices.lock().unwrap().clone();
    assert_eq!(
        lines,
        vec![client.startup_notice(), state_init_warning_line()],
        "the startup notice, then exactly one warning"
    );
    assert!(
        server.requests().is_empty(),
        "a failed init performs zero HTTP requests"
    );
    assert_eq!(
        persisted_artifacts(dir.path()),
        vec![INSTALLATION_ID_FILE.to_string(), OUTBOX_FILE.to_string()],
        "an initialization failure may be transient and shared with a concurrent \
         starter: it records nothing but deletes nothing"
    );
}

#[test]
fn start_never_waits_for_an_outbox_another_process_holds_locked() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let locker = rusqlite::Connection::open(dir.path().join(OUTBOX_FILE)).unwrap();
    locker
        .execute_batch("PRAGMA journal_mode = DELETE; CREATE TABLE held (x); BEGIN EXCLUSIVE;")
        .unwrap();

    let started = Instant::now();
    let client = client_for(&server.base_url, dir.path());
    let start_latency = started.elapsed();
    assert!(
        start_latency < Duration::from_secs(1),
        "start() waited {start_latency:?} on the locked outbox"
    );
    assert!(client.is_active());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    std::thread::sleep(SQLITE_BUSY_TIMEOUT + STATE_OPEN_RETRY_PAUSE);
    record_sample_op(&client, "ReceiveMessage", Outcome::Success);
    assert!(
        !dir.path().join(INSTALLATION_ID_FILE).exists(),
        "nothing is persisted while the outbox is locked"
    );
    assert!(server.requests().is_empty());

    locker.execute_batch("COMMIT;").unwrap();
    drop(locker);
    assert!(client.flush_blocking(WAIT));
    let bodies = server.ingest_bodies().join("\n");
    for operation in ["SendMessage", "ReceiveMessage"] {
        assert!(
            bodies.contains(operation),
            "{operation} recorded while the outbox was locked is delivered: {bodies}"
        );
    }
    client.shutdown(WAIT);
}

#[test]
fn shutdown_that_never_reaches_a_locked_worker_cancels_the_state_open() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let locker = rusqlite::Connection::open(dir.path().join(OUTBOX_FILE)).unwrap();
    locker
        .execute_batch("PRAGMA journal_mode = DELETE; CREATE TABLE held (x); BEGIN EXCLUSIVE;")
        .unwrap();

    let started = Instant::now();
    let client = client_for(&server.base_url, dir.path());
    for _ in 0..=RECORD_QUEUE_BOUND {
        record_sample_op(&client, "SendMessage", Outcome::Success);
    }
    let shutdown_timeout = Duration::from_millis(100);
    client.shutdown(shutdown_timeout);
    assert!(
        started.elapsed() < SQLITE_BUSY_TIMEOUT,
        "the shutdown gave up on the full queue instead of waiting out the lock"
    );

    std::thread::sleep(
        (SQLITE_BUSY_TIMEOUT + 3 * STATE_OPEN_RETRY_PAUSE).saturating_sub(started.elapsed()),
    );
    locker.execute_batch("COMMIT;").unwrap();
    drop(locker);
    std::thread::sleep(2 * STATE_OPEN_RETRY_PAUSE);

    assert!(
        !dir.path().join(INSTALLATION_ID_FILE).exists(),
        "no state is initialized once shutdown closed the gate"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn cross_host_ingest_option_fails_init() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.ingest_endpoint =
        Some(Endpoint::parse_ingest("https://elsewhere.example.com/v1/events").unwrap());
    let client = TelemetryClient::start(options);
    assert!(
        !client.is_active(),
        "an ingest endpoint on a foreign host must not start the client"
    );
    client.shutdown(Duration::from_secs(1));
    assert!(server.requests().is_empty());
}

/// A real listener that accepts connections and never answers, so the worker
/// wedges on its startup config fetch until reqwest's request timeout.
fn stalling_base_url() -> String {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept() {
            held.push(stream);
        }
    });
    format!("http://{addr}")
}

#[test]
fn a_full_queue_drops_records_without_blocking_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let client = client_for(&stalling_base_url(), dir.path());

    let started = std::time::Instant::now();
    for _ in 0..(RECORD_QUEUE_BOUND + 1000) {
        record_sample_op(&client, "QueuedOperation", Outcome::Success);
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "recording past the queue bound must drop, never block (took {:?})",
        started.elapsed()
    );
    client.shutdown(Duration::from_millis(100));
}

#[test]
fn flush_blocking_reports_failure_when_the_worker_cannot_finish() {
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let client = client_for(&stalling_base_url(), dir.path());
    record_sample_op(&client, "StalledOperation", Outcome::Success);
    assert!(
        !client.flush_blocking(Duration::from_millis(200)),
        "a flush that cannot complete inside its timeout reports false"
    );
    client.shutdown(Duration::from_millis(100));
}

#[test]
fn dropping_the_last_handle_triggers_a_final_flush() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    {
        let client = client_for(&server.base_url, dir.path());
        record_sample_op(&client, "FinalDropOperation", Outcome::Success);
    }
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline
        && !server
            .ingest_bodies()
            .iter()
            .any(|body| body.contains("FinalDropOperation"))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let bodies = server.ingest_bodies();
    let final_body = bodies
        .iter()
        .find(|body| body.contains("FinalDropOperation"))
        .expect("dropping the last handle still delivers pending metrics");
    let session: serde_json::Value =
        serde_json::from_str(final_body.lines().next().unwrap()).unwrap();
    assert_eq!(session["isFinal"], true, "a disconnect flush is final");
}

#[test]
fn request_timeout_408_is_retried_with_the_same_dedup_id() {
    let server = TestServer::start(&default_config_json());
    server
        .behavior
        .ingest_timeout_all
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "TimedOutOperation", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    server
        .behavior
        .ingest_timeout_all
        .store(false, Ordering::SeqCst);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    let bodies: Vec<_> = server
        .ingest_bodies()
        .into_iter()
        .filter(|body| body.contains("TimedOutOperation"))
        .collect();
    assert!(bodies.len() >= 2, "the 408 response schedules a retry");
    let batch_ids: std::collections::BTreeSet<_> = bodies
        .iter()
        .map(|body| {
            let session: serde_json::Value =
                serde_json::from_str(body.lines().next().unwrap()).unwrap();
            session["batchId"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(batch_ids.len(), 1, "a 408 retry keeps the dedup batch id");
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        0
    );
}

#[test]
fn forbidden_4xx_drops_the_batch_as_undeliverable() {
    let server = TestServer::start(&default_config_json());
    server
        .behavior
        .ingest_forbid_all
        .store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    record_sample_op(&client, "ForbiddenOperation", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    server
        .behavior
        .ingest_forbid_all
        .store(false, Ordering::SeqCst);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    let attempts = server
        .ingest_bodies()
        .into_iter()
        .filter(|body| body.contains("ForbiddenOperation"))
        .count();
    assert_eq!(
        attempts, 1,
        "a 403 batch is dropped as undeliverable, never retried"
    );
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        0
    );
}

#[test]
fn coverage_gap_metadata_reaches_the_wire() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let client = client_for(&server.base_url, dir.path());

    client.record_operation(
        &handle("sqs", "InternalTask"),
        Outcome::ServerError,
        Some(ErrorType::InternalError),
        Duration::from_millis(7),
    );
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::USER_AGENT,
        "boto3/1.34.100 python/3.12.1".parse().unwrap(),
    );
    client.record_coverage_gap_with_metadata(
        &handle("sqs", "NotThereYet"),
        sanitize_request_metadata(&headers, Region::new("eu-west-1").ok()),
    );
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    let body = server
        .ingest_bodies()
        .into_iter()
        .find(|body| body.contains("InternalTask"))
        .expect("the flush delivered both rows");
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let session = &lines[0];
    assert_eq!(session["environment"]["sdkName"], "boto3");
    assert_eq!(session["environment"]["sdkVersion"], "1.34.100");
    assert_eq!(session["environment"]["languageVersion"], "3.12.1");
    assert_eq!(session["environment"]["region"], "eu-west-1");

    let error_row = lines[1..]
        .iter()
        .find(|row| row["operation"] == "InternalTask")
        .expect("the failed-operation row");
    assert_eq!(error_row["type"], "operation");
    assert_eq!(error_row["outcome"], "server_error");
    assert_eq!(error_row["errorType"], "InternalError");
    assert!(error_row.get("latency").is_some());

    let gap_row = lines[1..]
        .iter()
        .find(|row| row["operation"] == "NotThereYet")
        .expect("the coverage-gap row");
    assert_eq!(gap_row["type"], "coverage_gap");
    assert_eq!(gap_row["outcome"], "not_emulated");
}

#[test]
fn metadata_conflicts_stick_mixed_for_the_whole_session() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let client = client_for(&server.base_url, dir.path());

    let mut rust_headers = http::HeaderMap::new();
    rust_headers.insert(
        http::header::USER_AGENT,
        "aws-sdk-rust/1.82.0 rust/1.85.1".parse().unwrap(),
    );
    let mut js_headers = http::HeaderMap::new();
    js_headers.insert(
        http::header::USER_AGENT,
        "aws-sdk-js-v3/3.900.0 nodejs/22.1.0".parse().unwrap(),
    );

    let record_with = |headers: &http::HeaderMap, region: &str| {
        client.record_operation_with_metadata(
            &handle("sqs", "SendMessage"),
            Outcome::Success,
            None,
            Duration::from_millis(5),
            sanitize_request_metadata(headers, Region::new(region).ok()),
        );
        assert!(client.flush_blocking(WAIT));
    };
    record_with(&rust_headers, "us-west-2");
    record_with(&js_headers, "eu-west-1");
    record_with(&rust_headers, "us-west-2");
    client.shutdown(WAIT);

    let sessions: Vec<serde_json::Value> = server
        .ingest_bodies()
        .iter()
        .map(|body| serde_json::from_str(body.lines().next().unwrap()).unwrap())
        .collect();
    assert!(sessions.len() >= 3);
    assert_eq!(sessions[0]["environment"]["sdkName"], "aws-sdk-rust");
    assert_eq!(sessions[0]["environment"]["sdkVersion"], "1.82.0");
    assert_eq!(sessions[0]["environment"]["languageVersion"], "1.85.1");
    assert_eq!(sessions[0]["environment"]["region"], "us-west-2");

    assert_eq!(sessions[1]["environment"]["sdkName"], "mixed");
    assert!(sessions[1]["environment"].get("sdkVersion").is_none());
    assert!(sessions[1]["environment"].get("languageVersion").is_none());
    assert!(sessions[1]["environment"].get("region").is_none());

    let last = sessions.last().unwrap();
    assert_eq!(
        last["environment"]["sdkName"], "mixed",
        "an agreeing later observation can never un-mix the session"
    );
    assert!(last["environment"].get("region").is_none());
}

#[test]
fn installing_the_panic_hook_on_an_inert_client_is_a_no_op() {
    let _serial = panic_hook_serial();
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(
        dir.path().join(EMULATOR_CONFIG_FILE_NAME),
        "[telemetry]\nenabled = false\n",
    )
    .unwrap();

    let client = client_for(&server.base_url, dir.path());
    assert!(!client.is_active());
    client.install_panic_hook();
    client.shutdown(Duration::from_secs(1));
    assert!(server.requests().is_empty());
    assert!(!dir.path().join(OUTBOX_FILE).exists());
}

#[test]
fn the_embedding_api_switch_is_an_equal_off_wins_source() {
    let server = TestServer::start(&default_config_json());

    let opted_out = tempfile::tempdir().unwrap();
    opt_in(opted_out.path());
    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(opted_out.path().to_path_buf());
    options.set_config_file_override(opted_out.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.embedding_opt_out = true;
    let client = TelemetryClient::start(options);
    assert!(
        !client.is_active(),
        "an embedding off wins over the config file's on"
    );
    let notice = client.startup_notice();
    assert!(
        notice.contains("via the embedding configuration"),
        "the notice names the deciding source: {notice:?}"
    );
    record_sample_op(&client, "SendMessage", Outcome::Success);
    client.shutdown(Duration::from_secs(1));
    assert!(server.requests().is_empty());
    assert!(!opted_out.path().join(OUTBOX_FILE).exists());

    let opted_in = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(opted_in.path()).unwrap();
    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(opted_in.path().to_path_buf());
    options.set_config_file_override(opted_in.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.set_retry_backoff_override(Duration::ZERO);
    options.embedding_opt_out = false;
    let client = TelemetryClient::start(options);
    assert_eq!(
        client.is_active(),
        emulator_telemetry_default_enabled(),
        "a silent embedding never enables: only user sources or the compiled \
         default can"
    );
    let notice = client.startup_notice();
    assert!(
        notice.contains("(on by default"),
        "the deciding source is the default, never the embedding: {notice:?}"
    );
    client.shutdown(WAIT);
}

#[test]
fn the_environment_endpoint_is_never_an_authority_without_a_stage_anchor() {
    let anchor = Endpoint::parse("https://telemetry.beta.example.com").unwrap();
    let same_host = "https://telemetry.beta.example.com/other";
    let cross_host = "https://attacker.example";

    assert_eq!(
        resolve_config_endpoint(Some(cross_host), None),
        None,
        "without an injected anchor the environment cannot introduce an endpoint"
    );
    assert_eq!(resolve_config_endpoint(None, None), None);
    assert_eq!(
        resolve_config_endpoint(Some(cross_host), Some(&anchor))
            .unwrap()
            .host(),
        anchor.host(),
        "a cross-host override is ignored in favor of the anchor"
    );
    assert_eq!(
        resolve_config_endpoint(Some("not a url"), Some(&anchor)).unwrap(),
        anchor,
        "an unparseable override falls back to the anchor"
    );
    assert_eq!(
        resolve_config_endpoint(None, Some(&anchor)).unwrap(),
        anchor
    );
    let overridden = resolve_config_endpoint(Some(same_host), Some(&anchor)).unwrap();
    assert_eq!(overridden.host(), anchor.host());
    assert_eq!(
        overridden.as_str(),
        "https://telemetry.beta.example.com/other"
    );
}

#[test]
fn a_client_without_an_injected_endpoint_is_inert_and_touches_no_network() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let mut options = TelemetryOptions::without_endpoint(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    let client = TelemetryClient::start(options);

    assert!(!client.is_active());
    assert!(
        client.startup_notice().contains("no stage endpoint"),
        "unexpected notice: {}",
        client.startup_notice()
    );
    record_sample_op(&client, "SendMessage", Outcome::Success);
    client.flush_blocking(Duration::from_secs(1));
    client.shutdown(Duration::from_secs(1));
    assert!(
        server.requests().is_empty(),
        "an endpointless client must never touch the network"
    );
}

#[test]
fn opting_out_deletes_the_existing_installation_id_and_outbox() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.is_active());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);
    assert!(
        dir.path().join(INSTALLATION_ID_FILE).exists(),
        "an opted-in run persists an installation id"
    );
    assert!(
        dir.path().join(OUTBOX_FILE).exists(),
        "an opted-in run persists an outbox"
    );

    std::fs::write(
        dir.path().join(EMULATOR_CONFIG_FILE_NAME),
        "[telemetry]\nenabled = false\n",
    )
    .unwrap();
    let opted_out = client_for(&server.base_url, dir.path());
    assert!(!opted_out.is_active());
    opted_out.shutdown(Duration::from_secs(1));
    assert!(
        !dir.path().join(INSTALLATION_ID_FILE).exists(),
        "opting out DELETES the stored installation id"
    );
    assert!(
        !dir.path().join(OUTBOX_FILE).exists(),
        "opting out DELETES the local outbox"
    );
    assert!(
        dir.path().join(EMULATOR_CONFIG_FILE_NAME).exists(),
        "the user's config file is never telemetry's to delete"
    );
}

#[test]
fn a_fail_safe_off_from_an_invalid_value_purges_the_existing_state() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.is_active());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);
    assert!(dir.path().join(INSTALLATION_ID_FILE).exists());

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.set_switch_env_override("onn");
    let failed_safe = TelemetryClient::start(options);
    assert!(
        !failed_safe.is_active(),
        "an invalid value fails safe to off"
    );
    assert_eq!(failed_safe.state_purge_error(), None);
    failed_safe.shutdown(Duration::from_secs(1));

    assert_no_persisted_state(dir.path(), "fail-safe off");
    assert!(
        dir.path().join(EMULATOR_CONFIG_FILE_NAME).exists(),
        "the user's config file is never telemetry's to delete"
    );
}

#[test]
fn every_effective_off_path_purges_the_existing_state() {
    let short_ttl =
        default_config_json().replace("\"configTtlSeconds\": 3600", "\"configTtlSeconds\": 1");
    let server = TestServer::start(&short_ttl);
    let disabled_server =
        TestServer::start(&short_ttl.replace("\"enabled\": true", "\"enabled\": false"));
    *disabled_server.behavior.etag.lock().unwrap() = "\"disabled\"".to_string();

    let seed = |dir: &Path| {
        opt_in(dir);
        let client = client_for(&server.base_url, dir);
        assert!(client.is_active());
        record_sample_op(&client, "SendMessage", Outcome::Success);
        assert!(client.flush_blocking(WAIT));
        client.shutdown(WAIT);
        assert!(dir.join(INSTALLATION_ID_FILE).exists());
        assert!(dir.join(OUTBOX_FILE).exists());
    };
    let options_for = |dir: &Path, endpoint: &str| {
        let mut options = TelemetryOptions::new(
            ProjectId::new(TEST_PROJECT_ID).unwrap(),
            ProductVersion::new("1.0").unwrap(),
            Endpoint::parse(endpoint).unwrap(),
        );
        options.state_dir = Some(dir.to_path_buf());
        options.set_config_file_override(dir.join(EMULATOR_CONFIG_FILE_NAME));
        options.set_retry_backoff_override(Duration::ZERO);
        options
    };

    let env_off = tempfile::tempdir().unwrap();
    seed(env_off.path());
    let mut options = options_for(env_off.path(), &server.base_url);
    options.set_switch_env_override(SWITCH_OFF);
    let client = TelemetryClient::start(options);
    assert!(!client.is_active());
    client.shutdown(WAIT);
    assert_no_persisted_state(env_off.path(), "env off");

    let embedding_off = tempfile::tempdir().unwrap();
    seed(embedding_off.path());
    let mut options = options_for(embedding_off.path(), &server.base_url);
    options.embedding_opt_out = true;
    let client = TelemetryClient::start(options);
    assert!(!client.is_active());
    client.shutdown(WAIT);
    assert_no_persisted_state(embedding_off.path(), "embedding opt-out");

    let invalid_file = tempfile::tempdir().unwrap();
    seed(invalid_file.path());
    std::fs::write(
        invalid_file.path().join(EMULATOR_CONFIG_FILE_NAME),
        "[telemetry]\nenabled = \"yes please\"\n",
    )
    .unwrap();
    let client = TelemetryClient::start(options_for(invalid_file.path(), &server.base_url));
    assert!(
        !client.is_active(),
        "an unparseable config value fails safe"
    );
    client.shutdown(WAIT);
    assert_no_persisted_state(invalid_file.path(), "invalid config-file value");

    let remote_off = tempfile::tempdir().unwrap();
    seed(remote_off.path());
    std::thread::sleep(Duration::from_millis(1100));
    let client = TelemetryClient::start(options_for(remote_off.path(), &disabled_server.base_url));
    assert!(client.flush_blocking(WAIT));
    assert_denied_for_the_session(&client, "remote off");
    client.shutdown(WAIT);
    assert_no_persisted_state(remote_off.path(), "remote off");

    let no_endpoint = tempfile::tempdir().unwrap();
    seed(no_endpoint.path());
    let mut options = TelemetryOptions::without_endpoint(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
    );
    options.state_dir = Some(no_endpoint.path().to_path_buf());
    options.set_config_file_override(no_endpoint.path().join(EMULATOR_CONFIG_FILE_NAME));
    let client = TelemetryClient::start(options);
    assert!(!client.is_active());
    client.shutdown(WAIT);
    assert_no_persisted_state(no_endpoint.path(), "no endpoint");
}

/// An embedding application that opts out while a client is running gets the
/// same purge as an opt-out at start: the installation id and the batches the
/// session queued are deleted, and nothing more is sent. A plain shutdown keeps
/// them, which is why the opt-out needs its own call.
#[test]
fn opting_out_a_running_client_purges_the_existing_state() {
    let server = TestServer::start(&default_config_json());
    server
        .behavior
        .ingest_fail_all
        .store(true, Ordering::SeqCst);
    let seed = |dir: &Path| {
        opt_in(dir);
        let client = client_for(&server.base_url, dir);
        assert!(client.is_active());
        record_sample_op(&client, "SendMessage", Outcome::Success);
        assert!(client.flush_blocking(WAIT));
        assert!(dir.join(INSTALLATION_ID_FILE).exists());
        assert!(dir.join(OUTBOX_FILE).exists());
        client
    };

    let shut_down = tempfile::tempdir().unwrap();
    seed(shut_down.path()).shutdown(Duration::ZERO);
    assert!(
        shut_down.path().join(INSTALLATION_ID_FILE).exists()
            && shut_down.path().join(OUTBOX_FILE).exists(),
        "a shutdown alone keeps the local state"
    );

    let opted_out = tempfile::tempdir().unwrap();
    let client = seed(opted_out.path());
    server
        .behavior
        .ingest_fail_all
        .store(false, Ordering::SeqCst);
    let ingest_posts_before = server.ingest_bodies().len();
    record_sample_op(&client, "SendMessage", Outcome::Success);
    client.opt_out();

    assert!(!client.recording_active());
    assert_eq!(client.state_purge_error(), None);
    assert_no_persisted_state(opted_out.path(), "opt-out of a running client");
    assert_eq!(
        server.ingest_bodies().len(),
        ingest_posts_before,
        "an opt-out sends nothing, not even the final flush"
    );
    assert!(
        opted_out.path().join(EMULATOR_CONFIG_FILE_NAME).exists(),
        "the user's config file is never telemetry's to delete"
    );
    client.opt_out();
    assert_no_persisted_state(opted_out.path(), "a repeated opt-out");
}

/// The server's decision must outlive the purge it triggers: the next start,
/// with the config endpoint unreachable, reads the cached decision and stays
/// denied instead of falling back to the compiled (enabled) defaults.
#[test]
fn a_server_denial_stays_in_force_offline_after_the_purge() {
    let cases = [
        (
            "remote disabled",
            default_config_json().replace("\"enabled\": true", "\"enabled\": false"),
        ),
        (
            "unsupported client version",
            default_config_json().replace(
                &format!("\"minSupportedClientVersion\": \"{CLIENT_VERSION}\""),
                "\"minSupportedClientVersion\": \"99.0.0\"",
            ),
        ),
        (
            "unsampled",
            default_config_json().replace("\"samplingRate\": 1.0", "\"samplingRate\": 0.0"),
        ),
    ];
    for (context, config) in cases {
        let server = TestServer::start(&config);
        let dir = tempfile::tempdir().unwrap();
        opt_in(dir.path());

        let online = client_for(&server.base_url, dir.path());
        assert!(online.flush_blocking(WAIT));
        assert_denied_for_the_session(&online, context);
        online.shutdown(WAIT);
        assert!(
            dir.path().join(SERVER_CONFIG_CACHE_FILE).exists(),
            "{context}: the server's decision survives the purge"
        );

        let offline = client_for(&refused_base_url(), dir.path());
        record_sample_op(&offline, "SendMessage", Outcome::Success);
        assert!(offline.flush_blocking(WAIT));
        assert_denied_for_the_session(&offline, &format!("{context}, offline restart"));
        offline.shutdown(WAIT);

        assert_eq!(
            server.ingest_bodies().len(),
            0,
            "{context}: nothing was ever posted"
        );
        assert_no_persisted_state(dir.path(), &format!("{context}, offline restart"));
    }
}

/// The crash reporter is gated exactly like every other write: nothing before
/// the decision, nothing after a denial, and nothing while the denial purge
/// holds the gate — so a panic can never recreate deleted state.
#[test]
fn a_crash_record_is_written_only_while_collection_is_allowed() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.flush_blocking(WAIT), "the decision has landed");
    let pending_before = Outbox::open(dir.path()).unwrap().pending_count().unwrap();
    {
        let _purge_in_progress = client.hold_purge_lock_for_test();
        client.report_crash_for_test();
    }
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        pending_before,
        "a crash racing the purge is dropped, never written"
    );
    client.report_crash_for_test();
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        pending_before + 1,
        "an allowed session durably queues the crash record"
    );
    client.shutdown(WAIT);

    let disabled_server = TestServer::start(
        &default_config_json().replace("\"enabled\": true", "\"enabled\": false"),
    );
    let denied_dir = tempfile::tempdir().unwrap();
    opt_in(denied_dir.path());
    let denied = client_for(&disabled_server.base_url, denied_dir.path());
    assert!(denied.flush_blocking(WAIT));
    assert_denied_for_the_session(&denied, "denied session");
    denied.report_crash_for_test();
    denied.shutdown(WAIT);
    assert_no_persisted_state(
        denied_dir.path(),
        "a crash after the denial recreates nothing",
    );
}

/// The panic hook writes through a replaceable slot: once a client shuts
/// down, or an effectively-off client takes over the process, the earlier
/// reporter is retired and a later panic writes nothing anywhere.
#[test]
fn a_retired_crash_reporter_writes_nothing() {
    let _serial = panic_hook_serial();
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.flush_blocking(WAIT));
    client.install_panic_hook();
    client.shutdown(WAIT);
    let pending_after_shutdown = Outbox::open(dir.path()).unwrap().pending_count().unwrap();
    CrashHook::report_for_test();
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        pending_after_shutdown,
        "shutdown retires the reporter: a later panic queues nothing"
    );

    let active = client_for(&server.base_url, dir.path());
    assert!(active.flush_blocking(WAIT));
    active.install_panic_hook();
    let mut off = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    off.state_dir = Some(dir.path().to_path_buf());
    off.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    off.set_switch_env_override(SWITCH_OFF);
    let inert = TelemetryClient::start(off);
    assert!(!inert.is_active());
    inert.install_panic_hook();
    assert_no_persisted_state(dir.path(), "the effectively-off client purged");
    CrashHook::report_for_test();
    assert_no_persisted_state(
        dir.path(),
        "the retired reporter of the still-live earlier client writes nothing",
    );
    active.shutdown(WAIT);
}

/// Before the worker's first decision the gate is pending: callers may
/// record (their observations wait in memory), but a crash arriving then is
/// dropped, never written — the reporter is not fail-open.
#[test]
fn a_crash_before_the_collection_decision_is_dropped() {
    let server = TestServer::start(&default_config_json());
    let release = server.hold_config_responses();
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    server.wait_for_first_request();
    assert!(
        client.recording_active(),
        "callers may record while the decision is pending"
    );
    client.report_crash_for_test();
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        0,
        "a crash before the decision is dropped, never written"
    );

    drop(release);
    assert!(client.flush_blocking(WAIT), "the decision has landed");
    let pending_before = Outbox::open(dir.path()).unwrap().pending_count().unwrap();
    client.report_crash_for_test();
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        pending_before + 1,
        "once allowed, the crash record is durably queued"
    );
    client.shutdown(WAIT);
}

/// Shutdown closes the collection gate for good: a reporter handle a panic
/// hook upgraded a moment earlier writes nothing, and asking the shut-down
/// client to install the hook again cannot re-arm it.
#[test]
fn shutdown_closes_the_gate_so_no_later_crash_is_written() {
    let _serial = panic_hook_serial();
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let client = client_for(&server.base_url, dir.path());
    assert!(client.flush_blocking(WAIT));
    client.install_panic_hook();
    assert!(client.recording_active());
    client.shutdown(WAIT);
    assert!(
        !client.recording_active(),
        "a shut-down client records nothing"
    );
    assert!(!client.is_active());

    let pending_after_shutdown = Outbox::open(dir.path()).unwrap().pending_count().unwrap();
    client.report_crash_for_test();
    client.install_panic_hook();
    CrashHook::report_for_test();
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        pending_after_shutdown,
        "neither a retained reporter handle nor a re-installed hook writes after shutdown"
    );
}

/// A shutdown that gives up on a worker still blocked in its startup fetch
/// closes the gate for good: when the fetch finally answers "enabled", the
/// late decision cannot reopen collection, drain the outbox, send anything,
/// or arm the crash reporter.
#[test]
fn a_shutdown_that_outlives_a_slow_startup_stays_closed() {
    let _serial = panic_hook_serial();
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    server
        .behavior
        .ingest_fail_all
        .store(true, Ordering::SeqCst);
    let earlier = client_for(&server.base_url, dir.path());
    record_sample_op(&earlier, "SendMessage", Outcome::Success);
    assert!(earlier.flush_blocking(WAIT));
    earlier.shutdown(WAIT);
    server
        .behavior
        .ingest_fail_all
        .store(false, Ordering::SeqCst);
    let queued = Outbox::open(dir.path()).unwrap().pending_count().unwrap();
    assert!(queued > 0, "an earlier session left records to drain");
    std::fs::remove_file(dir.path().join(SERVER_CONFIG_CACHE_FILE)).unwrap();

    let release = server.hold_config_responses();
    let accepted_before = server.accepted_connections();
    let client = client_for(&server.base_url, dir.path());
    server.wait_for_accepted(accepted_before + 1);
    assert!(client.recording_active(), "the decision is still pending");

    let posts_before = server.ingest_bodies().len();
    let config_requests_before = server.config_requests().len();
    client.shutdown(Duration::from_millis(200));
    assert!(!client.recording_active(), "shutdown closed the gate");
    assert!(!client.is_active());

    drop(release);
    let started = Instant::now();
    while server.config_requests().len() == config_requests_before {
        assert!(
            started.elapsed() < WAIT,
            "the held config request was answered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(
        server.ingest_bodies().len(),
        posts_before,
        "the late 'enabled' decision drains and sends nothing"
    );
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        queued,
        "the outbox is neither drained nor purged by a shutdown"
    );
    assert!(!client.recording_active(), "a closed gate never reopens");
    client.report_crash_for_test();
    client.install_panic_hook();
    CrashHook::report_for_test();
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        queued,
        "no crash record is written through the closed gate"
    );
}

/// A shutdown that gives up on a worker already authorized and mid-drain
/// closes the gate; the request in flight completes, but delivery stops at
/// the next batch boundary — the remaining rows stay queued for a later
/// session instead of being sent after the client was closed.
#[test]
fn a_shutdown_that_outlives_an_authorized_drain_stops_at_the_next_batch() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    server
        .behavior
        .ingest_fail_all
        .store(true, Ordering::SeqCst);
    let earlier = client_for(&server.base_url, dir.path());
    for operation in ["SendMessage", "ReceiveMessage", "DeleteMessage"] {
        record_sample_op(&earlier, operation, Outcome::Success);
        assert!(earlier.flush_blocking(WAIT));
    }
    earlier.shutdown(WAIT);
    server
        .behavior
        .ingest_fail_all
        .store(false, Ordering::SeqCst);
    let queued = Outbox::open(dir.path()).unwrap().pending_count().unwrap();
    assert!(
        queued >= 3,
        "an earlier session left several batches to drain"
    );
    std::fs::remove_file(dir.path().join(SERVER_CONFIG_CACHE_FILE)).unwrap();

    let release = server.hold_ingest_responses();
    let posts_before = server.ingest_bodies().len();
    let accepted_before = server.accepted_connections();
    let client = client_for(&server.base_url, dir.path());
    server.wait_for_accepted(accepted_before + 2);
    assert!(
        client.recording_active(),
        "the server allowed collection and the first queued batch is in flight"
    );

    client.shutdown(Duration::from_millis(200));
    assert!(!client.recording_active(), "shutdown closed the gate");

    drop(release);
    let started = Instant::now();
    while server.ingest_bodies().len() == posts_before {
        assert!(
            started.elapsed() < WAIT,
            "the held ingest request was answered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    while Outbox::open(dir.path()).unwrap().pending_count().unwrap() == queued {
        assert!(
            started.elapsed() < WAIT,
            "the batch already in flight is acknowledged and removed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(
        server.ingest_bodies().len(),
        posts_before + 1,
        "only the request already in flight completed; nothing else was sent after shutdown"
    );
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        queued - 1,
        "the remaining batches stay queued for a later session"
    );
    assert!(!client.recording_active(), "a closed gate never reopens");
}

/// A cache that exists but cannot be read is never mistaken for "no cache":
/// offline, the session collects nothing (rather than falling back to the
/// shipped defaults over a decision it cannot read), deletes nothing (the
/// server's wishes are unknown), and says so exactly once.
#[test]
fn an_unreadable_cache_while_offline_stops_the_session_without_deleting_anything() {
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let installation_id = persist_installation_id(dir.path());
    let cache_path = dir.path().join(SERVER_CONFIG_CACHE_FILE);
    std::fs::write(&cache_path, "{not json").unwrap();
    let notices: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&refused_base_url()).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    let sink = notices.clone();
    options.set_notice_sink_override(Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    }));
    let client = TelemetryClient::start(options);
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    assert!(client.flush_blocking(WAIT));

    assert!(
        !client.recording_active(),
        "no decision means no collection"
    );
    assert!(!client.is_active());
    let error = client
        .config_cache_error()
        .expect("the unreadable cache is observable");
    assert_eq!(error.kind, std::io::ErrorKind::InvalidData);
    assert!(client.state_purge_error().is_none());
    let lines = notices.lock().unwrap().clone();
    let warnings: Vec<&String> = lines
        .iter()
        .filter(|line| *line == &config_cache_warning_line(&error))
        .collect();
    assert_eq!(warnings.len(), 1, "warned exactly once: {lines:?}");
    assert!(!warnings[0].contains(dir.path().to_str().unwrap()));

    client.shutdown(WAIT);
    assert_eq!(
        std::fs::read_to_string(&cache_path).unwrap(),
        "{not json",
        "nothing was written over the unreadable cache"
    );
    assert!(
        dir.path().join(OUTBOX_FILE).exists(),
        "nothing is deleted when the server's wishes are unknown: {:?}",
        persisted_artifacts(dir.path())
    );
    assert_eq!(
        persisted_installation_id(dir.path()),
        Some(installation_id),
        "the persisted installation id is kept"
    );
    assert_eq!(
        Outbox::open(dir.path()).unwrap().pending_count().unwrap(),
        0,
        "nothing was recorded without a decision"
    );
}

/// A failure to persist the server's decision is surfaced exactly once —
/// typed, in the notice, path-free — while collection keeps following the
/// live decision. It is never silently ignored.
#[test]
fn a_failed_config_cache_write_surfaces_as_a_typed_error_and_a_notice_warning() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    std::fs::create_dir(dir.path().join(SERVER_CONFIG_CACHE_FILE)).unwrap();
    let notices: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    let sink = notices.clone();
    options.set_notice_sink_override(Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    }));
    let client = TelemetryClient::start(options);
    assert!(client.flush_blocking(WAIT));
    assert!(client.flush_blocking(WAIT));
    assert!(
        client.recording_active(),
        "collection follows the live decision, not the cache"
    );

    let error = client
        .config_cache_error()
        .expect("the cache write failure is observable");
    assert!(client.state_purge_error().is_none());
    let lines = notices.lock().unwrap().clone();
    let warnings: Vec<&String> = lines
        .iter()
        .filter(|line| *line == &config_cache_warning_line(&error))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "the warning is emitted exactly once across refreshes: {lines:?}"
    );
    assert!(
        !warnings[0].contains(dir.path().to_str().unwrap()),
        "the warning never echoes the absolute state path: {:?}",
        warnings[0]
    );
    client.shutdown(WAIT);
    let bodies = server.ingest_bodies();
    assert!(!bodies.is_empty(), "an allowed session still delivers");
    assert!(
        bodies.last().unwrap().contains("\"isFinal\":true"),
        "the final flush is delivered from the session's own decision when the cache is unreadable: {bodies:?}"
    );
}

/// A deletion failure inside the worker (after a server-side denial) is
/// surfaced exactly like one at startup: typed, in the notice, path-free —
/// and one undeletable artifact never shields the others.
#[test]
fn a_failed_worker_side_purge_surfaces_and_still_deletes_the_rest() {
    let server = TestServer::start(
        &default_config_json().replace("\"enabled\": true", "\"enabled\": false"),
    );
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let blocker = dir.path().join(format!("{OUTBOX_FILE}.blocker"));
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("pin"), b"keeps the directory non-empty").unwrap();
    let notices: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    let sink = notices.clone();
    options.set_notice_sink_override(Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    }));
    let client = TelemetryClient::start(options);
    assert!(client.flush_blocking(WAIT));
    assert!(!client.recording_active(), "the server denied collection");

    let error = client
        .state_purge_error()
        .expect("the worker-side purge failure is observable");
    assert!(
        matches!(
            error,
            StatePurgeError::Remove {
                artifact: StateArtifact::Outbox,
                ..
            }
        ),
        "the error names the artifact that could not be removed: {error:?}"
    );
    let lines = notices.lock().unwrap().clone();
    let warning = lines
        .iter()
        .find(|line| *line == &state_purge_warning_line(&error))
        .unwrap_or_else(|| panic!("the worker emitted the warning line: {lines:?}"));
    assert!(
        !warning.contains(dir.path().to_str().unwrap()),
        "the warning never echoes the absolute state path: {warning:?}"
    );
    client.shutdown(WAIT);

    assert_eq!(
        persisted_artifacts(dir.path()),
        vec![format!("{OUTBOX_FILE}.blocker")],
        "everything deletable was deleted despite the failure"
    );
    assert!(server.ingest_bodies().is_empty());
}

#[test]
fn a_failed_state_purge_surfaces_as_a_typed_error_and_a_notice_warning() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    drop(Outbox::open(dir.path()).unwrap());
    assert!(
        persisted_artifacts(dir.path())
            .iter()
            .any(|name| name.starts_with(OUTBOX_FILE)),
        "an outbox exists before the failed purge"
    );
    let undeletable = dir.path().join(INSTALLATION_ID_FILE);
    std::fs::create_dir(&undeletable).unwrap();
    std::fs::write(undeletable.join("pin"), b"keeps the directory non-empty").unwrap();

    std::fs::write(
        dir.path().join(EMULATOR_CONFIG_FILE_NAME),
        "[telemetry]\nenabled = false\n",
    )
    .unwrap();
    let client = client_for(&server.base_url, dir.path());
    assert!(!client.is_active());

    let error = client
        .state_purge_error()
        .expect("a purge that could not delete the installation id is reported");
    assert!(
        matches!(
            error,
            StatePurgeError::Remove {
                artifact: StateArtifact::InstallationId,
                ..
            }
        ),
        "the error names the artifact that survived: {error:?}"
    );
    let notice = client.startup_notice();
    let mut lines = notice.lines();
    assert!(
        lines.next().unwrap_or_default().contains("disabled"),
        "the notice line names the off state: {notice:?}"
    );
    let warning = lines.next().unwrap_or_default();
    assert_eq!(
        warning,
        state_purge_warning_line(&error),
        "the warning line carries the purge failure: {notice:?}"
    );
    assert_eq!(lines.next(), None, "one notice line plus one warning line");
    assert!(
        !warning.contains(dir.path().to_str().unwrap()),
        "the warning never echoes the absolute state path: {warning:?}"
    );
    client.shutdown(Duration::from_secs(1));
    assert!(
        server.requests().is_empty(),
        "an off client with a failed purge still never touches the network"
    );
    assert_eq!(
        persisted_artifacts(dir.path()),
        vec![INSTALLATION_ID_FILE.to_string()],
        "the outbox was still deleted; only the undeletable artifact remains"
    );
}

#[test]
fn nothing_is_transmitted_before_the_startup_notice() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let notice_at: Arc<Mutex<Vec<(Instant, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.set_retry_backoff_override(Duration::ZERO);
    let sink_log = notice_at.clone();
    options.set_notice_sink_override(Arc::new(move |line: &str| {
        sink_log
            .lock()
            .unwrap()
            .push((Instant::now(), line.to_string()));
    }));
    let client = TelemetryClient::start(options);
    assert!(client.is_active());
    record_sample_op(&client, "SendMessage", Outcome::Success);
    assert!(client.flush_blocking(WAIT));
    client.shutdown(WAIT);

    let notices = notice_at.lock().unwrap();
    assert_eq!(
        notices.len(),
        1,
        "start emits the startup notice exactly once"
    );
    let (notice_instant, line) = &notices[0];
    assert!(line.contains("collects usage and error telemetry: enabled"));
    let first_request = server
        .first_request_at()
        .expect("the flush reached the server");
    assert!(
        *notice_instant <= first_request,
        "nothing is transmitted before the startup notice is emitted"
    );
}

#[test]
fn an_invalid_switch_env_value_fails_safe_with_a_one_line_warning() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.set_switch_env_override("maybe");
    let client = TelemetryClient::start(options);

    assert!(
        !client.is_active(),
        "an unrecognized switch value fails safe to off even over a file opt-in"
    );
    let notice = client.startup_notice();
    let mut lines = notice.lines();
    assert!(
        lines.next().unwrap_or_default().contains("failing safe"),
        "the notice line names the fail-safe: {notice:?}"
    );
    assert_eq!(
        lines.next(),
        Some(invalid_switch_warning_line("maybe").as_str()),
        "the warning line echoes the invalid value: {notice:?}"
    );
    assert_eq!(lines.next(), None, "one notice line plus one warning line");
    client.shutdown(Duration::from_secs(1));
    assert!(server.requests().is_empty());
}

#[test]
fn switch_tokens_reach_the_client_case_insensitively() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();

    let mut options = TelemetryOptions::new(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
        Endpoint::parse(&server.base_url).unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.set_retry_backoff_override(Duration::ZERO);
    options.set_switch_env_override("TRUE");
    let client = TelemetryClient::start(options);
    assert!(
        client.is_active(),
        "TRUE is an accepted case-insensitive enable token"
    );
    client.shutdown(WAIT);
}

#[test]
fn the_embedded_run_mode_notice_goes_to_the_tracing_target() {
    struct CapturingSubscriber {
        events: Arc<Mutex<Vec<(String, String)>>>,
    }
    impl tracing::Subscriber for CapturingSubscriber {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            struct MessageVisitor(String);
            impl tracing::field::Visit for MessageVisitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);
            self.events
                .lock()
                .unwrap()
                .push((event.metadata().target().to_string(), visitor.0));
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    let dir = tempfile::tempdir().unwrap();
    let mut options = TelemetryOptions::without_endpoint(
        ProjectId::new(TEST_PROJECT_ID).unwrap(),
        ProductVersion::new("1.0").unwrap(),
    );
    options.state_dir = Some(dir.path().to_path_buf());
    options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
    options.set_switch_env_override(SWITCH_OFF);
    options.run_mode = RunMode::Embedded;

    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        events: events.clone(),
    };
    let client = tracing::subscriber::with_default(subscriber, || TelemetryClient::start(options));
    let events = events.lock().unwrap();
    let notices: Vec<_> = events
        .iter()
        .filter(|(target, _)| target == NOTICE_LOG_TARGET)
        .collect();
    assert_eq!(notices.len(), 1, "one startup notice in the host log");
    assert!(
        notices[0].1.contains(&client.startup_notice()),
        "the startup notice reaches the host log: {notices:?}"
    );
}

const PROPERTY_SEED: u64 = 0x0b5e_55ed_c011_ec70;
const PROPERTY_ITERATIONS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedCollection {
    Collecting,
    LocallyOff,
    RemotelyOff,
    Unsampled,
}

#[derive(Debug)]
struct RandomSession {
    local_switch: Option<&'static str>,
    returning_installation: Option<InstallationId>,
    server_enabled: bool,
    sampling_rate: f64,
    min_client_version: &'static str,
    namespace: String,
    operation: String,
    error_shape: String,
    http_status: u16,
}

impl RandomSession {
    fn draw(rng: &mut FuzzRng) -> Self {
        RandomSession {
            local_switch: *rng.pick(&[None, None, Some(SWITCH_ON), Some(SWITCH_OFF), Some("onn")]),
            returning_installation: rng.chance(2).then(InstallationId::random),
            server_enabled: !rng.chance(3),
            sampling_rate: *rng.pick(&[1.0, 1.0, 0.0, 0.5]),
            min_client_version: rng.pick(&[CLIENT_VERSION, CLIENT_VERSION, "99.0.0"]),
            namespace: rng.string(),
            operation: rng.string(),
            error_shape: rng.string(),
            http_status: rng.below(1000) as u16,
        }
    }

    fn config_json(&self) -> String {
        default_config_json()
            .replace(
                "\"enabled\": true",
                &format!("\"enabled\": {}", self.server_enabled),
            )
            .replace(
                "\"samplingRate\": 1.0",
                &format!("\"samplingRate\": {:.1}", self.sampling_rate),
            )
            .replace(
                &format!("\"minSupportedClientVersion\": \"{CLIENT_VERSION}\""),
                &format!(
                    "\"minSupportedClientVersion\": \"{}\"",
                    self.min_client_version
                ),
            )
    }

    /// A fresh installation's id is only known once it is persisted, which
    /// happens exactly when it is sampled; an unsampled one is certain only
    /// at rate zero, so a fresh start at a partial rate with no id persisted
    /// is classed unsampled and must then have left nothing behind.
    fn expected(&self, persisted: Option<&InstallationId>) -> ExpectedCollection {
        if matches!(self.local_switch, Some(SWITCH_OFF) | Some("onn")) {
            return ExpectedCollection::LocallyOff;
        }
        if !self.server_enabled || self.min_client_version != CLIENT_VERSION {
            return ExpectedCollection::RemotelyOff;
        }
        match self.returning_installation.as_ref().or(persisted) {
            Some(id) if crate::config::is_sampled(id, self.sampling_rate) => {
                ExpectedCollection::Collecting
            }
            Some(_) => ExpectedCollection::Unsampled,
            None if self.sampling_rate >= 1.0 => ExpectedCollection::Collecting,
            None => ExpectedCollection::Unsampled,
        }
    }
}

/// Owner rule, end to end: whatever the local switch, the server config and
/// the recorded strings, the wire carries only allowlisted values, and every
/// state that is not "collecting" leaves nothing queued and nothing sent (an
/// unsampled returning installation keeps only its installation id; a fresh
/// one writes none).
#[test]
fn random_config_states_gate_collection_and_leave_no_records_when_denied() {
    println!("collection-gate property: seed={PROPERTY_SEED:#x} iterations={PROPERTY_ITERATIONS}");
    let mut rng = FuzzRng(PROPERTY_SEED);
    let mut seen = std::collections::BTreeSet::new();
    for iteration in 0..PROPERTY_ITERATIONS {
        let session = RandomSession::draw(&mut rng);
        let server = TestServer::start(&session.config_json());
        let dir = tempfile::tempdir().unwrap();
        opt_in(dir.path());
        if let Some(id) = &session.returning_installation {
            std::fs::write(
                dir.path().join(INSTALLATION_ID_FILE),
                format!("{}\n", id.as_str()),
            )
            .unwrap();
        }
        let mut options = TelemetryOptions::new(
            ProjectId::new(TEST_PROJECT_ID).unwrap(),
            ProductVersion::new("1.0").unwrap(),
            Endpoint::parse(&server.base_url).unwrap(),
        );
        options.state_dir = Some(dir.path().to_path_buf());
        options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
        options.set_retry_backoff_override(Duration::ZERO);
        if let Some(value) = session.local_switch {
            options.set_switch_env_override(value);
        }
        let client = TelemetryClient::start(options);

        let handle = OperationHandle::from_dimensions(
            Namespace::fold(&session.namespace),
            OperationName::fold(&session.operation),
        );
        dispatch_generated_operation(
            &client,
            &handle,
            Duration::from_millis(3),
            Some(OperationFailure::classify(
                session.http_status,
                &session.error_shape,
            )),
            None,
        );
        client.record_coverage_gap(&handle);
        assert!(
            client.flush_blocking(WAIT),
            "iteration {iteration}: flush acknowledged"
        );
        client.shutdown(WAIT);

        let persisted = persisted_installation_id(dir.path());
        let expected = session.expected(persisted.as_ref());
        seen.insert(format!("{expected:?}"));
        let bodies = server.ingest_bodies();
        let context = format!("iteration {iteration} ({expected:?}, {session:?})");
        assert_eq!(
            client.state_purge_error(),
            None,
            "{context}: purge succeeded"
        );
        match expected {
            ExpectedCollection::Collecting => {
                assert!(!bodies.is_empty(), "{context}: a collecting session posts");
                for body in &bodies {
                    crate::backend_contract::validate_ingest_body(body)
                        .unwrap_or_else(|reason| panic!("{context}: {reason}\n{body}"));
                    for raw in [&session.namespace, &session.operation, &session.error_shape] {
                        assert!(
                            raw.is_empty()
                                || Namespace::new(raw).is_ok()
                                || !body.contains(raw.as_str()),
                            "{context}: a raw string outside the dimension grammar reached \
                             the wire: {raw:?}"
                        );
                    }
                }
            }
            ExpectedCollection::LocallyOff | ExpectedCollection::RemotelyOff => {
                assert!(bodies.is_empty(), "{context}: nothing is posted while off");
                assert!(!client.recording_active(), "{context}: recording is off");
                assert_no_persisted_state(dir.path(), &context);
            }
            ExpectedCollection::Unsampled => {
                assert!(
                    bodies.is_empty(),
                    "{context}: an unsampled session posts nothing"
                );
                assert!(!client.recording_active(), "{context}: recording is off");
                if session.returning_installation.is_some() {
                    assert_only_the_installation_id_persists(dir.path(), &context);
                } else {
                    assert_no_persisted_state(dir.path(), &context);
                }
            }
        }
        assert!(
            dir.path().join(EMULATOR_CONFIG_FILE_NAME).exists(),
            "{context}: the user's config file survives every state"
        );
    }
    assert_eq!(
        seen.len(),
        4,
        "the seed must exercise every collection state: {seen:?}"
    );
}

/// Run `body` on a thread driving the given tokio runtime, the way a
/// generated emulator's `main` runs `build_and_serve` under `block_on`, and
/// fail the test with the panic message if anything inside it panics.
fn inside_tokio_runtime(runtime: tokio::runtime::Runtime, body: impl FnOnce()) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async { body() });
    }));
    if let Err(payload) = outcome {
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| text.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        panic!("telemetry panicked inside an async runtime: {message}");
    }
}

fn start_record_flush_and_shut_down_inside(runtime: tokio::runtime::Runtime) {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    inside_tokio_runtime(runtime, || {
        let client = client_for(&server.base_url, dir.path());
        assert!(client.is_active(), "{}", client.startup_notice());
        record_sample_op(&client, "InsideRuntime", Outcome::Success);
        assert!(client.flush_blocking(WAIT));
        client.shutdown(WAIT);
    });
    assert!(
        server
            .ingest_bodies()
            .iter()
            .any(|body| body.contains("InsideRuntime")),
        "a client started inside the runtime still delivers"
    );
}

#[test]
fn starts_and_shuts_down_inside_a_multi_thread_tokio_runtime() {
    start_record_flush_and_shut_down_inside(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap(),
    );
}

#[test]
fn starts_and_shuts_down_inside_a_current_thread_tokio_runtime() {
    start_record_flush_and_shut_down_inside(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
    );
}

#[test]
fn dropping_the_last_handle_inside_a_tokio_runtime_does_not_panic() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());

    inside_tokio_runtime(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap(),
        || {
            let client = client_for(&server.base_url, dir.path());
            assert!(client.is_active(), "{}", client.startup_notice());
            record_sample_op(&client, "DroppedInsideRuntime", Outcome::Success);
            drop(client);
        },
    );
}

#[test]
fn init_failure_after_the_http_client_exists_does_not_panic_inside_a_tokio_runtime() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let not_a_directory = dir.path().join("occupied");
    std::fs::write(&not_a_directory, "a file where the state dir should be").unwrap();

    inside_tokio_runtime(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap(),
        || {
            let mut options = TelemetryOptions::new(
                ProjectId::new(TEST_PROJECT_ID).unwrap(),
                ProductVersion::new("1.0").unwrap(),
                Endpoint::parse(&server.base_url).unwrap(),
            );
            options.state_dir = Some(not_a_directory.join("state"));
            options.set_config_file_override(dir.path().join(EMULATOR_CONFIG_FILE_NAME));
            let client = TelemetryClient::start(options);
            assert!(
                client.flush_blocking(WAIT),
                "a client whose state failed to open acknowledges flushes"
            );
            assert!(
                !client.is_active(),
                "an unusable state dir leaves the client inert"
            );
            client.shutdown(WAIT);
        },
    );
    assert!(
        server.requests().is_empty(),
        "an inert client sends nothing"
    );
}

/// The child half of [`process_exit_sends_the_final_batch`]: records one operation through the
/// global client and returns, so only the exit hook can deliver it.
#[test]
#[ignore = "run by process_exit_sends_the_final_batch in a child process"]
fn child_records_one_operation_then_exits() {
    let (Ok(base), Ok(dir)) = (
        std::env::var("EXIT_HOOK_SERVER"),
        std::env::var("EXIT_HOOK_DIR"),
    ) else {
        return;
    };
    let client = init_global(client_for(&base, Path::new(&dir)));
    crate::flush_at_process_exit();
    record_sample_op(client, "SendMessage", Outcome::Success);
}

#[test]
fn process_exit_sends_the_final_batch() {
    let server = TestServer::start(&default_config_json());
    let dir = tempfile::tempdir().unwrap();
    opt_in(dir.path());
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "integration_tests::child_records_one_operation_then_exits",
        ])
        .env("EXIT_HOOK_SERVER", &server.base_url)
        .env("EXIT_HOOK_DIR", dir.path())
        .status()
        .unwrap();
    assert!(status.success());
    let bodies = server.ingest_bodies();
    assert!(
        bodies
            .iter()
            .any(|b| b.contains("\"isFinal\":true") && b.contains("SendMessage")),
        "the exit hook delivered the recorded operation in a final batch: {bodies:?}"
    );
}
