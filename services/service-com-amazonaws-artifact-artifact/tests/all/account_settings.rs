// SPDX-License-Identifier: CC0-1.0

//! AWS Artifact's account settings, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones the account-settings surface states:
//!
//! * the account settings are PERSISTED: `GetAccountSettings` reports the
//!   `notificationSubscriptionStatus` the last `PutAccountSettings` wrote, whatever the
//!   value and however many writes preceded it, and the enum's opt-out value before any
//!   such call;
//! * `GetAccountSettings` is a reporting operation, so reading the settings any number of
//!   times leaves what a later caller sees unchanged;
//! * every WRITE of the settings leaves one structured, attributable record on the
//!   `emulator::audit` target: the operation, the access key id the request presented,
//!   the credential scope's region, the status now stored and the outcome;
//! * a desired status outside the set `NotificationSubscriptionStatus` declares never
//!   reaches the store: the gateway this service is served through refuses the body with
//!   HTTP 400 `BadRequestException` (the front-door rule `front_door_errors.rs` pins),
//!   and the stored status is left as it was, while a request naming no desired status
//!   asks for no change.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `NotificationSubscriptionStatus` declares, spelled out here rather than
/// imported from the crate so a wrong set in `src/ops/types/shared_types.rs` fails this
/// suite instead of agreeing with it.
///
/// Source: the model's `com.amazonaws.artifact#NotificationSubscriptionStatus` enum —
/// `smithy_query(query="shape", shape="NotificationSubscriptionStatus")`.
const SUBSCRIPTION_STATUS_VALUES: &[&str] = &["NOT_SUBSCRIBED", "SUBSCRIBED"];

/// The status an account reports before any `PutAccountSettings` call: AWS Artifact
/// notifications are opt-in — subscribing is what `PutAccountSettings` is for — so a
/// fresh account carries the enum's opt-out value.
const DEFAULT_STATUS: &str = "NOT_SUBSCRIBED";

/// A SigV4 credential scope naming this service; it is what routes a request to the
/// artifact service.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `accountSettings.notificationSubscriptionStatus` the answer carries, if any.
    fn reported_status(&self) -> Option<&str> {
        self.body["accountSettings"]["notificationSubscriptionStatus"].as_str()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    call_signed(router, method, uri, body, SIGNED_FOR_EU_WEST_1).await
}

async fn call_signed(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    authorization: &str,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `GetAccountSettings` — `GET /v1/account-settings/get`.
async fn get_account_settings(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/v1/account-settings/get", None).await
}

/// `PutAccountSettings` — `PUT /v1/account-settings/put`, with `body` as its payload.
async fn put_account_settings(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "PUT", "/v1/account-settings/put", Some(body)).await
}

/// The status `GetAccountSettings` reports, asserted to be a modelled enum value.
async fn read_status(router: &MultiServiceRouter, case: &str) -> String {
    let answer = get_account_settings(router).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetAccountSettings must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = answer.reported_status().unwrap_or_else(|| {
        panic!(
            "case {case:?}: GetAccountSettings must report an accountSettings with a \
             notificationSubscriptionStatus, got {:?}",
            answer.body
        )
    });
    assert!(
        SUBSCRIPTION_STATUS_VALUES.contains(&reported),
        "case {case:?}: the reported status must be one of \
         {SUBSCRIPTION_STATUS_VALUES:?}, got {reported:?}"
    );
    reported.to_string()
}

#[tokio::test]
async fn a_fresh_account_reports_the_opt_out_default_and_reading_it_changes_nothing() {
    let router = emulator();

    // Read it repeatedly: a reporting operation may not move the record it reports.
    for read in 0..3 {
        let case = format!("read {read} of a fresh account");
        let reported = read_status(&router, &case).await;
        assert_eq!(
            reported, DEFAULT_STATUS,
            "case {case:?}: an account nobody has subscribed must report \
             {DEFAULT_STATUS:?}, got {reported:?}"
        );
    }
}

/// One write sequence: what a caller puts, in order.
struct Sequence {
    label: &'static str,
    /// The `notificationSubscriptionStatus` of each successive `PutAccountSettings`.
    writes: &'static [&'static str],
}

/// The sequences under test. The rows differ along what the persistence rule generalises
/// over: which value is written, how many writes precede the read, and whether successive
/// writes repeat a value (idempotence) or change it (a toggle). Between them they write
/// every value `NotificationSubscriptionStatus` declares — pinned by
/// [`every_modelled_status_is_written_by_some_sequence`].
const SEQUENCES: &[Sequence] = &[
    Sequence {
        label: "subscribe",
        writes: &["SUBSCRIBED"],
    },
    Sequence {
        label: "write the default back over itself",
        writes: &["NOT_SUBSCRIBED"],
    },
    Sequence {
        label: "subscribe twice",
        writes: &["SUBSCRIBED", "SUBSCRIBED"],
    },
    Sequence {
        label: "subscribe then unsubscribe",
        writes: &["SUBSCRIBED", "NOT_SUBSCRIBED"],
    },
    Sequence {
        label: "unsubscribe then subscribe",
        writes: &["NOT_SUBSCRIBED", "SUBSCRIBED"],
    },
    Sequence {
        label: "toggle three times",
        writes: &["SUBSCRIBED", "NOT_SUBSCRIBED", "SUBSCRIBED"],
    },
];

#[tokio::test]
async fn a_read_reports_the_status_the_last_write_persisted() {
    for sequence in SEQUENCES {
        // A fresh emulator per row, so each sequence starts from the default.
        let router = emulator();
        let case = sequence.label;

        for (step, written) in sequence.writes.iter().enumerate() {
            let answer =
                put_account_settings(&router, json!({"notificationSubscriptionStatus": written}))
                    .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} write {step}: PutAccountSettings must answer HTTP 200, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.reported_status(),
                Some(*written),
                "case {case:?} write {step}: PutAccountSettings must return the status it \
                 persisted, got {:?}",
                answer.body
            );

            // Every intermediate read sees the write that preceded it, not just the last.
            let reported = read_status(&router, &format!("{case} after write {step}")).await;
            assert_eq!(
                reported, *written,
                "case {case:?} write {step}: GetAccountSettings must report the persisted \
                 status, got {reported:?}"
            );
        }

        let last = sequence.writes.last().copied().unwrap_or(DEFAULT_STATUS);
        let reported = read_status(&router, &format!("{case} re-read")).await;
        assert_eq!(
            reported, last,
            "case {case:?}: a later read must still report the last written status, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_status_is_written_by_some_sequence() {
    for value in SUBSCRIPTION_STATUS_VALUES {
        assert!(
            SEQUENCES
                .iter()
                .any(|sequence| sequence.writes.contains(value)),
            "case {value:?}: every NotificationSubscriptionStatus value must be covered by \
             a row of SEQUENCES, which is missing {value:?}"
        );
    }
}

/// Desired statuses that are NOT values of `NotificationSubscriptionStatus`: the wrong
/// case, a truncation, a plausible synonym, the empty string, padding, and a value from
/// another of this service's enums.
const STATUSES_OUTSIDE_THE_ENUM: &[&str] = &[
    "subscribed",
    "Subscribed",
    "SUBSCRIBE",
    "UNSUBSCRIBED",
    "",
    " SUBSCRIBED",
    "SUBSCRIBED ",
    "PUBLISHED",
];

#[tokio::test]
async fn a_status_outside_the_enum_never_reaches_the_store() {
    for rejected in STATUSES_OUTSIDE_THE_ENUM {
        let router = emulator();

        // Put the account in the NON-default state first, so a write that was wrongly
        // accepted — or wrongly ignored — cannot be mistaken for the default.
        let accepted = put_account_settings(
            &router,
            json!({"notificationSubscriptionStatus": "SUBSCRIBED"}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {rejected:?}: the setup write must answer HTTP 200, got {} with {:?}",
            accepted.status, accepted.body
        );

        let answer =
            put_account_settings(&router, json!({"notificationSubscriptionStatus": rejected}))
                .await;
        assert_eq!(
            answer.status, 400,
            "case {rejected:?}: a status outside the modelled enum must be refused with \
             HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        // This service is served through a gateway that refuses a body it cannot bind to
        // the modelled shape before the service is reached, so the refusal is the
        // gateway's `BadRequestException` rather than the service's own
        // `ValidationException` — the rule `tests/all/front_door_errors.rs` pins.
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {rejected:?}: the refusal must carry the front door's \
             BadRequestException code, got {:?}",
            answer.body
        );

        let reported = read_status(&router, rejected).await;
        assert_eq!(
            reported, "SUBSCRIBED",
            "case {rejected:?}: a refused write must leave the stored status untouched, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_write_naming_no_status_leaves_the_stored_status_unchanged() {
    // `notificationSubscriptionStatus` is not modelled `@required`, so a request naming no
    // desired status is admissible and asks for no change. Run it from both of the states
    // the account can be in, so "unchanged" is not satisfied by always answering one value.
    for stored in SUBSCRIPTION_STATUS_VALUES {
        let router = emulator();
        let setup =
            put_account_settings(&router, json!({"notificationSubscriptionStatus": stored})).await;
        assert_eq!(
            setup.status, 200,
            "case {stored:?}: the setup write must answer HTTP 200, got {} with {:?}",
            setup.status, setup.body
        );

        let answer = put_account_settings(&router, json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {stored:?}: a write naming no status must answer HTTP 200, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.reported_status(),
            Some(*stored),
            "case {stored:?}: a write naming no status must report the unchanged stored \
             status, got {:?}",
            answer.body
        );

        let reported = read_status(&router, stored).await;
        assert_eq!(
            reported, *stored,
            "case {stored:?}: a write naming no status must leave the stored status \
             unchanged, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every write leaves a structured, attributable record
// ---------------------------------------------------------------------------

/// A `tracing` subscriber that renders each event as one `target field=value …` line and
/// keeps the lines so a test can read them back.
///
/// Written against `tracing`'s own `Subscriber` trait rather than pulling in a formatting
/// subscriber crate — the same harness this tree's other audit suites use (e.g.
/// `service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs`)
/// — so this change adds no dependency.
#[derive(Clone, Default)]
struct CapturedRecords(Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// Every record captured since the last drain, clearing the buffer.
    fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("the audit capture lock"))
    }
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The access key id the audit case signs with, so it can pick its own records out of a
/// process-wide log that sibling tests also write to.
const AUDIT_AKID: &str = "AKIDAUDITSETTINGS";

/// The region the audit case signs for — different from [`REGION_EU_WEST_1`] on purpose,
/// so the record's `region` has to come from the request's credential scope rather than
/// from a constant.
const AUDIT_REGION: &str = "ap-southeast-2";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for, which every other case here uses.
const REGION_EU_WEST_1: &str = "eu-west-1";

/// The capture for audit records, installed ONCE as this test binary's global `tracing`
/// subscriber.
///
/// Global rather than scoped on purpose: `tracing` caches per callsite whether any
/// subscriber is interested, the cache is process-wide, and sibling tests run in parallel
/// on other threads. [`AUDIT_AKID`] is what keeps a sibling's records out of this one's
/// assertions.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// The captured records that are THIS case's: on the `emulator::audit` target and
/// presenting [`AUDIT_AKID`].
fn audit_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with("emulator::audit ") && record.contains(&format!("akid={AUDIT_AKID}"))
        })
        .cloned()
        .collect()
}

#[tokio::test]
async fn every_write_emits_a_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing audit
    // record: the capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", akid = AUDIT_AKID, "probe");
    let probe = audit_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={AUDIT_AKID}/20260101/{AUDIT_REGION}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );

    // (case, request body, the status the record must report, whether it changed)
    let cases: [(&str, Value, &str, bool); 4] = [
        (
            "subscribe",
            json!({"notificationSubscriptionStatus": "SUBSCRIBED"}),
            "SUBSCRIBED",
            true,
        ),
        (
            "subscribe again",
            json!({"notificationSubscriptionStatus": "SUBSCRIBED"}),
            "SUBSCRIBED",
            true,
        ),
        (
            "unsubscribe",
            json!({"notificationSubscriptionStatus": "NOT_SUBSCRIBED"}),
            "NOT_SUBSCRIBED",
            true,
        ),
        (
            "a write naming no status",
            json!({}),
            "NOT_SUBSCRIBED",
            false,
        ),
    ];

    for (case, body, status, changed) in cases {
        let answer = call_signed(
            &router,
            "PUT",
            "/v1/account-settings/put",
            Some(body),
            &authorization,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the write must answer HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );

        let records = audit_records(&captured.drain());
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: one write must leave exactly one audit record, got {records:?}"
        );
        let record = &records[0];
        for (field, value) in [
            ("operation", "PutAccountSettings".to_string()),
            ("akid", AUDIT_AKID.to_string()),
            ("region", AUDIT_REGION.to_string()),
            ("notification_subscription_status", status.to_string()),
            ("changed", changed.to_string()),
            ("outcome", "recorded".to_string()),
        ] {
            // Asserted as the `field=value` pair, not as a bare substring, so a value
            // cannot satisfy the check from the wrong field.
            let pair = format!("{field}={value}");
            assert!(
                record.contains(&pair),
                "case {case:?}: the audit record must carry {pair:?}, got {record:?}"
            );
        }
        assert!(
            !record.contains(REGION_EU_WEST_1),
            "case {case:?}: the record's region must come from the request's credential \
             scope, got {record:?}"
        );
    }
}
