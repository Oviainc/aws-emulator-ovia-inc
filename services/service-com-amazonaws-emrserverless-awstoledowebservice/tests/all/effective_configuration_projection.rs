// SPDX-License-Identifier: CC0-1.0

//! The EFFECTIVE-CONFIGURATION projection a read reports, end to end over HTTP.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! The rule under test: a read of an application or of an interactive session reports
//! the configuration the service APPLIED to it — every configuration block present, each
//! carrying the value the create or update named or the documented default the service
//! filled in, under every key the service's wire response carries for it — and the
//! endpoint a session is reached at is reported as the origin a client connects to.
//! Three suites, each over a SET of inputs rather than over one recorded request:
//!
//! 1. `GetApplication` reports the whole projection, for an application created with
//!    none of the blocks and for one created with each of them at a different value, and
//!    `UpdateApplication` leaves the same projection behind.
//! 2. `GetSession` always reports a session's extended-support flag, as a boolean.
//! 3. `GetSessionEndpoint` reports an https ORIGIN naming the session under the EMR
//!    Serverless session-endpoint domain of the region the call was signed for: no port,
//!    no path, no query.

use serde_json::{json, Value};

use super::application_lifecycle::{
    call, create_application, emulator, get_application, minimal_create, wait_for_transition,
    Answer,
};
use super::session_lifecycle::{get_session, minimal_session, session_application, start_session};
use super::update_application::update_application;

/// The region every test request is signed for (`application_lifecycle`'s credential
/// scope), which is the region a reported endpoint names.
const REGION: &str = "eu-west-1";

/// The host form an EMR Serverless interactive session is reached at: the session's own
/// id, this domain, the region and the AWS suffix. Written out here rather than read from
/// `crate::state`, so a wrong constant in `src/` cannot make this test pass.
const SESSION_ENDPOINT_DOMAIN: &str = "s.emr-serverless-services";

/// `AutoStopConfig$idleTimeoutMinutes` is `@range(min: 1, max: 10080)` and documents
/// "Defaults to 15 minutes" (`smithy_query(query="shape",shape="AutoStopConfig")`).
const DEFAULT_IDLE_TIMEOUT_MINUTES: i64 = 15;
const MIN_IDLE_TIMEOUT_MINUTES: i64 = 1;
const MAX_IDLE_TIMEOUT_MINUTES: i64 = 10080;

/// A KMS key ARN satisfying `EncryptionKeyArn`'s pattern
/// (`smithy_query(query="shape",shape="ManagedPersistenceMonitoringConfiguration")`).
const KEY_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:key/00000000-0000-0000-0000-000000000001";

/// The extended-support projection every application reports while nothing has changed
/// the account's setting: off, set at the account, never changed.
fn expected_extended_support() -> Value {
    json!({
        "enabled": false,
        "source": "ACCOUNT",
        "updatedAt": null,
        "updatedBy": null,
    })
}

/// One row of the application table: what the create asks for, and the effective
/// configuration a read must then report.
struct ApplicationCase {
    case: &'static str,
    /// The configuration members the create names, beside the required ones.
    configuration: Value,
    /// `autoStartConfiguration.enabled`.
    auto_start: bool,
    /// `autoStopConfiguration.enabled`.
    auto_stop: bool,
    /// `autoStopConfiguration.idleTimeoutMinutes`, and the same value under the other
    /// name the block carries it under.
    idle_timeout: i64,
    /// `monitoringConfiguration.managedPersistenceMonitoringConfiguration.enabled`.
    managed_persistence: bool,
    /// `monitoringConfiguration.managedPersistenceMonitoringConfiguration.encryptionKeyArn`,
    /// which has no default: absent unless the create named one.
    managed_persistence_key: Option<&'static str>,
    /// `jobLevelCostAllocationConfiguration.enabled`.
    cost_allocation: bool,
}

/// The rows: an application created with none of these blocks, and one per block at a
/// value that is not its default — including both published bounds of the idle timeout,
/// each `enabled` flag inverted, and the key the managed-persistence block admits.
fn application_cases() -> Vec<ApplicationCase> {
    vec![
        ApplicationCase {
            case: "none of the configuration blocks",
            configuration: json!({}),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "auto-start and auto-stop both off",
            configuration: json!({
                "autoStartConfiguration": { "enabled": false },
                "autoStopConfiguration": { "enabled": false },
            }),
            auto_start: false,
            auto_stop: false,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "the idle timeout at its published minimum",
            configuration: json!({
                "autoStopConfiguration": { "idleTimeoutMinutes": MIN_IDLE_TIMEOUT_MINUTES },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: MIN_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "the idle timeout at its published maximum",
            configuration: json!({
                "autoStopConfiguration": { "idleTimeoutMinutes": MAX_IDLE_TIMEOUT_MINUTES },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: MAX_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "a monitoring configuration naming another of its blocks",
            configuration: json!({
                "monitoringConfiguration": {
                    "s3MonitoringConfiguration": { "logUri": "s3://bucket/logs" },
                },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "managed log persistence turned off",
            configuration: json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": { "enabled": false },
                },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: false,
            managed_persistence_key: None,
            cost_allocation: false,
        },
        ApplicationCase {
            case: "managed log persistence with an encryption key",
            configuration: json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": {
                        "encryptionKeyArn": KEY_ARN,
                    },
                },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: Some(KEY_ARN),
            cost_allocation: false,
        },
        ApplicationCase {
            case: "job level cost allocation turned on",
            configuration: json!({
                "jobLevelCostAllocationConfiguration": { "enabled": true },
            }),
            auto_start: true,
            auto_stop: true,
            idle_timeout: DEFAULT_IDLE_TIMEOUT_MINUTES,
            managed_persistence: true,
            managed_persistence_key: None,
            cost_allocation: true,
        },
        ApplicationCase {
            case: "every block at once, each away from its default",
            configuration: json!({
                "autoStartConfiguration": { "enabled": false },
                "autoStopConfiguration": { "enabled": true, "idleTimeoutMinutes": 120 },
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": {
                        "enabled": false,
                        "encryptionKeyArn": KEY_ARN,
                    },
                },
                "jobLevelCostAllocationConfiguration": { "enabled": true },
            }),
            auto_start: false,
            auto_stop: true,
            idle_timeout: 120,
            managed_persistence: false,
            managed_persistence_key: Some(KEY_ARN),
            cost_allocation: true,
        },
    ]
}

/// The `CreateApplication` body for one row.
fn create_for(case: &ApplicationCase, token: &str) -> Value {
    let mut body = minimal_create(token);
    for (member, value) in case.configuration.as_object().into_iter().flatten() {
        body[member.as_str()] = value.clone();
    }
    body
}

/// Assert the whole effective-configuration projection one row expects, over whichever
/// answer carries the application (`GetApplication`'s or `UpdateApplication`'s).
fn assert_projection(label: &str, source: &str, answer: &Answer, case: &ApplicationCase) {
    let expected_auto_stop = json!({
        "enabled": case.auto_stop,
        "idleTimeoutMinutes": case.idle_timeout,
        "idleTimeout": case.idle_timeout,
    });
    let expected_auto_start = json!({ "enabled": case.auto_start });
    let mut expected_managed = json!({ "enabled": case.managed_persistence });
    if let Some(key) = case.managed_persistence_key {
        expected_managed["encryptionKeyArn"] = json!(key);
    }

    for (pointer, expected) in [
        // Both spellings of the auto-start block, carrying the same setting.
        ("/application/autoStartConfiguration", expected_auto_start.clone()),
        ("/application/autoStartConfig", expected_auto_start),
        // Both spellings of the auto-stop block, each carrying the idle timeout under
        // both of ITS names.
        ("/application/autoStopConfiguration", expected_auto_stop.clone()),
        ("/application/autoStopConfig", expected_auto_stop),
        (
            "/application/monitoringConfiguration/managedPersistenceMonitoringConfiguration",
            expected_managed,
        ),
        (
            "/application/jobLevelCostAllocationConfiguration",
            json!({ "enabled": case.cost_allocation }),
        ),
        (
            "/application/extendedSupportConfiguration",
            expected_extended_support(),
        ),
    ] {
        assert_eq!(
            answer.at(pointer),
            expected,
            "case {label:?}: {source}{pointer} expected {expected:?}, got {:?}",
            answer.at(pointer)
        );
    }
}

/// Suite 1: every read of an application reports the configuration the service applied,
/// under every key its response carries — and an update leaves the same projection.
#[tokio::test]
async fn a_read_reports_the_applications_effective_configuration() {
    for (index, case) in application_cases().into_iter().enumerate() {
        let router = emulator();
        let token = format!("tok-projection-{index}");
        let created = create_application(&router, create_for(&case, &token)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateApplication expected 200, got {} with {:?}",
            case.case, created.status, created.body
        );
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();

        let fetched = get_application(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: GetApplication expected 200, got {} with {:?}",
            case.case, fetched.status, fetched.body
        );
        assert_projection(case.case, "GetApplication", &fetched, &case);

        // The same projection survives an update that names nothing of it: the record is
        // what carries the configuration, so the reader reports it again unchanged.
        wait_for_transition();
        let updated = update_application(
            &router,
            &id,
            json!({ "clientToken": format!("upd-{index}"), "releaseLabel": "emr-7.13.0" }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: UpdateApplication expected 200, got {} with {:?}",
            case.case, updated.status, updated.body
        );
        assert_projection(case.case, "UpdateApplication", &updated, &case);
        let read_back = get_application(&router, &id).await;
        assert_projection(case.case, "GetApplication after the update", &read_back, &case);
    }
}

/// Suite 1b: an update that REPLACES a configuration block gets the same defaults
/// materialised into it, under every key, as a create does.
#[tokio::test]
async fn an_update_reports_the_blocks_it_replaces_in_full() {
    // (case, the block the update names, the idle timeout that then applies, whether
    // managed log persistence is then on)
    let cases: Vec<(&str, Value, i64, bool)> = vec![
        (
            "an auto-stop block naming only the timeout",
            json!({ "autoStopConfiguration": { "idleTimeoutMinutes": MIN_IDLE_TIMEOUT_MINUTES } }),
            MIN_IDLE_TIMEOUT_MINUTES,
            true,
        ),
        (
            "an auto-stop block naming only the flag",
            json!({ "autoStopConfiguration": { "enabled": true } }),
            DEFAULT_IDLE_TIMEOUT_MINUTES,
            true,
        ),
        (
            "an empty monitoring configuration",
            json!({ "monitoringConfiguration": {} }),
            DEFAULT_IDLE_TIMEOUT_MINUTES,
            true,
        ),
        (
            "a monitoring configuration turning managed log persistence off",
            json!({
                "monitoringConfiguration": {
                    "managedPersistenceMonitoringConfiguration": { "enabled": false },
                },
            }),
            DEFAULT_IDLE_TIMEOUT_MINUTES,
            false,
        ),
    ];

    for (index, (case, block, idle_timeout, managed_persistence)) in
        cases.into_iter().enumerate()
    {
        let router = emulator();
        let created =
            create_application(&router, minimal_create(&format!("tok-replace-{index}"))).await;
        let id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        wait_for_transition();

        let mut body = json!({ "clientToken": format!("repl-{index}") });
        for (member, value) in block.as_object().into_iter().flatten() {
            body[member.as_str()] = value.clone();
        }
        let updated = update_application(&router, &id, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateApplication expected 200, got {} with {:?}",
            updated.status, updated.body
        );

        for (pointer, expected) in [
            (
                "/application/autoStopConfiguration/idleTimeoutMinutes",
                json!(idle_timeout),
            ),
            (
                "/application/autoStopConfiguration/idleTimeout",
                json!(idle_timeout),
            ),
            ("/application/autoStopConfig/idleTimeout", json!(idle_timeout)),
            (
                "/application/monitoringConfiguration/managedPersistenceMonitoringConfiguration/enabled",
                json!(managed_persistence),
            ),
            (
                "/application/jobLevelCostAllocationConfiguration/enabled",
                json!(false),
            ),
        ] {
            assert_eq!(
                updated.at(pointer),
                expected,
                "case {case:?}: UpdateApplication{pointer} expected {expected:?}, got {:?}",
                updated.at(pointer)
            );
        }

        // ROUND TRIP: the reader reports exactly what the update answered.
        let read_back = get_application(&router, &id).await;
        assert_eq!(
            read_back.at("/application"),
            updated.at("/application"),
            "case {case:?}: GetApplication must report the updated record, got {:?} against \
             {:?}",
            read_back.at("/application"),
            updated.at("/application")
        );
    }
}

/// Suite 2: `GetSession` reports the session's extended-support flag, as a boolean,
/// whatever the start asked for and whenever the read arrives.
#[tokio::test]
async fn a_read_reports_a_sessions_extended_support_flag() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-session-projection").await;

    // The rows differ along what the start named and WHEN the read arrives: inside the
    // span the clock still owes the session its started state, and after it.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("only the required members", minimal_session("tok-flag-0"), false),
        (
            "a named session, read after it comes up",
            {
                let mut body = minimal_session("tok-flag-1");
                body["name"] = json!("projection");
                body
            },
            true,
        ),
        (
            "a session carrying its own idle timeout",
            {
                let mut body = minimal_session("tok-flag-2");
                body["idleTimeoutMinutes"] = json!(30);
                body
            },
            false,
        ),
        (
            "a session carrying tags, read after it comes up",
            {
                let mut body = minimal_session("tok-flag-3");
                body["tags"] = json!({ "team": "data" });
                body
            },
            true,
        ),
    ];

    for (case, body, settle) in cases {
        let started = start_session(&router, &application_id, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSession expected 200, got {} with {:?}",
            started.status, started.body
        );
        let session_id = started
            .at("/sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if settle {
            wait_for_transition();
        }

        let fetched = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetSession expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        let flag = fetched.at("/session/extendedSupportEnabled");
        assert_eq!(
            flag,
            json!(false),
            "case {case:?}: GetSession must report the extended-support flag as a boolean, \
             got {flag:?}"
        );
    }
}

/// Suite 3: the endpoint reported for a session is an https ORIGIN naming that session
/// under the session-endpoint domain of the region the call was signed for — no port, no
/// path, no query — and two sessions never share one.
#[tokio::test]
async fn a_session_endpoint_is_an_origin_naming_the_session() {
    let router = emulator();
    let (shared_application, _) = session_application(&router, "tok-origin-app").await;

    // The rows differ along which application the session runs on and whether the read
    // arrives before or after the clock owed the session its started state — neither of
    // which the host form depends on.
    let cases: Vec<(&str, bool, bool)> = vec![
        ("on a shared application, read straight away", false, false),
        ("on a shared application, read once started", false, true),
        ("on an application of its own", true, false),
        ("on an application of its own, read once started", true, true),
    ];

    let mut seen: Vec<(String, String)> = Vec::new();
    for (index, (case, own_application, settle)) in cases.into_iter().enumerate() {
        let application_id = if own_application {
            session_application(&router, &format!("tok-origin-app-{index}"))
                .await
                .0
        } else {
            shared_application.clone()
        };
        let started = start_session(
            &router,
            &application_id,
            minimal_session(&format!("tok-origin-{index}")),
        )
        .await;
        let session_id = started
            .at("/sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if settle {
            wait_for_transition();
        }

        let answer = call(
            &router,
            "GET",
            &format!("/applications/{application_id}/sessions/{session_id}/endpoint"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetSessionEndpoint expected 200, got {} with {:?}",
            answer.status, answer.body
        );

        let endpoint = answer
            .at("/endpoint")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected =
            format!("https://{session_id}.{SESSION_ENDPOINT_DOMAIN}.{REGION}.amazonaws.com");
        assert_eq!(
            endpoint, expected,
            "case {case:?}: endpoint expected {expected:?}, got {endpoint:?}"
        );
        // An ORIGIN: everything after the scheme is the host, so a client that splits
        // the value on `/` gets a host it can put in an SNI or `Host` header.
        let after_scheme = endpoint.strip_prefix("https://").unwrap_or(&endpoint);
        assert!(
            !after_scheme.contains('/')
                && !after_scheme.contains(':')
                && !after_scheme.contains('?'),
            "case {case:?}: the endpoint carries no port, path or query, got {endpoint:?}"
        );
        assert!(
            after_scheme.starts_with(&format!("{session_id}.")),
            "case {case:?}: the host's first label names the session, got {endpoint:?}"
        );

        seen.push((session_id, endpoint));
    }

    for (index, (session_id, endpoint)) in seen.iter().enumerate() {
        for (other_id, other_endpoint) in seen.iter().skip(index + 1) {
            assert_ne!(
                endpoint, other_endpoint,
                "sessions {session_id:?} and {other_id:?} must not share an endpoint, got \
                 {endpoint:?} twice"
            );
        }
    }
}
