// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopImport` over the CloudTrail import registry.
//!
//! Every case goes over the wire: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: CloudTrail_20131101.StopImport` through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is
//! what a client sees — the HTTP status, the error code and the body.
//!
//! The rules under test:
//!
//! * a stop moves an import that is still RUNNING (`INITIALIZING`, `IN_PROGRESS`) to
//!   `STOPPED` and stamps `UpdatedTimestamp`, and leaves an import that already
//!   reached a terminal status (`COMPLETED`, `FAILED`, `STOPPED`) exactly as stored —
//!   the five rows below are the whole `ImportStatus` enum
//!   (`smithy_query(query="shape",shape="ImportStatus")`), so the table is exhaustive
//!   over the input space and a generator would add nothing;
//! * the reply is computed from the stored record (ids, destinations, source and
//!   timestamps are the persisted ones), and a second stop reports the same record:
//!   the stop is idempotent and what it wrote survives the call;
//! * an import belongs to the Region it was created in, so an id naming no import in
//!   the caller's Region is refused exactly like an id naming no import at all;
//! * that refusal is `ImportNotFoundException` with the status the real service
//!   answers ([`IMPORT_NOT_FOUND_HTTP_STATUS`]) and a message naming the id from the
//!   request ([`import_not_found_message`]);
//! * an `ImportId` outside the modelled `UUID` constraint (length 36..36, pattern
//!   `^[a-f0-9\-]+$`, from `smithy_query(query="operation",operation="StopImport")`)
//!   is refused at the boundary and persists nothing.
//!
//! Which `X-Amz-Target` spellings reach a handler at all is this service's own rule
//! and is covered by `tests/all/target_routing.rs` and
//! `tests/all/target_prefix_dispatch.rs`, so it is not re-asserted here.
//!
//! The registry is seeded directly here, through the crate's public `state` module,
//! rather than by calling `StartImport`: a seeded record is exactly what that
//! handler inserts — the same registry, the same key — and seeding is what lets one
//! case per `ImportStatus` value start from a status no sequence of calls can
//! reach (`COMPLETED`, `FAILED`). `StartImport`'s own rules are covered by
//! `tests/all/import_start.rs`.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    import_not_found_message, ComAmazonawsCloudtrailCloudtrail20131101State, ImportRecord,
    IMPORT_NOT_FOUND_HTTP_STATUS, IMPORT_STATUS_COMPLETED, IMPORT_STATUS_FAILED,
    IMPORT_STATUS_INITIALIZING, IMPORT_STATUS_IN_PROGRESS, IMPORT_STATUS_STOPPED,
};

/// The `X-Amz-Target` prefix from the model's `awsJson1_1` service shape id.
const TARGET_PREFIX: &str = "CloudTrail_20131101.";

/// The Region most cases call in.
const HOME_REGION: &str = "us-east-1";

/// A second Region, to pin that an import is reachable only from its own.
const OTHER_REGION: &str = "us-west-2";

/// The length the model's `UUID` shape pins `ImportId` to (length 36..36).
const IMPORT_ID_LEN: usize = 36;

/// The instant a seeded import was created at, so every timestamp assertion has a
/// fixed value to compare against (epoch seconds, as `awsJson1_1` carries a `Date`).
const SEEDED_AT_EPOCH_SECONDS: u64 = 1_700_000_000;

/// The emulator under test: one shared engine — kept so a case can seed the import
/// registry and read it back — and the production router.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type`), or `""` for a success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            // A `__type` may be namespaced (`com.amazonaws.cloudtrail#Foo`); the
            // code clients branch on is the shape name.
            .map(|t| t.rsplit('#').next().unwrap_or(t))
            .unwrap_or_default()
    }

    /// Assert this reply is the modelled failure `code` with HTTP `status`.
    fn expect_error(&self, case: &str, status: u16, code: &str) -> &Value {
        assert_eq!(
            (self.status, self.error_code()),
            (status, code),
            "case {case}: expected HTTP {status} {code}, got {self:?}"
        );
        &self.body
    }

    /// Assert this reply is a success, and hand back its body.
    fn expect_ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {self:?}"
        );
        &self.body
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's;
        // pinning it off keeps these cases independent of how the process was
        // started.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Call `operation` from `region` with `body`.
    async fn call_in(&self, operation: &str, region: &str, body: &Value) -> Reply {
        // The Region a request is served in comes from its SigV4 credential scope,
        // which is also how the router resolves the service.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/cloudtrail/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Reply { status, body }
    }

    /// `StopImport` for `import_id`, called from `region`.
    async fn stop_import(&self, import_id: &str, region: &str) -> Reply {
        self.call_in("StopImport", region, &json!({ "ImportId": import_id }))
            .await
    }

    /// Put `record` in `region`'s share of the import registry — what `StartImport`
    /// will do once it is implemented.
    fn seed(&self, region: &str, record: &ImportRecord) {
        self.engine
            .state::<ComAmazonawsCloudtrailCloudtrail20131101State>()
            .lock()
            .imports
            .insert((record.import_id.clone(), region.to_string()), record.clone());
    }

    /// The stored record for `import_id` in `region`, read straight from the
    /// registry.
    fn stored(&self, import_id: &str, region: &str) -> Option<ImportRecord> {
        self.engine
            .state::<ComAmazonawsCloudtrailCloudtrail20131101State>()
            .lock()
            .import(import_id, region)
            .cloned()
    }

    /// How many imports the registry holds, in every Region.
    fn import_count(&self) -> usize {
        self.engine
            .state::<ComAmazonawsCloudtrailCloudtrail20131101State>()
            .lock()
            .imports
            .len()
    }
}

/// A seeded import: `import_id`, `status`, and the modelled members `StartImport`
/// would have been given.
fn import_record(import_id: &str, status: &str) -> ImportRecord {
    assert_eq!(
        import_id.len(),
        IMPORT_ID_LEN,
        "case {import_id:?}: a seeded import id must satisfy the modelled length {IMPORT_ID_LEN}, \
         got {}",
        import_id.len()
    );
    let created = UNIX_EPOCH + Duration::from_secs(SEEDED_AT_EPOCH_SECONDS);
    ImportRecord {
        import_id: import_id.to_string(),
        destinations: vec![format!(
            "arn:aws:cloudtrail:{HOME_REGION}:123456789012:eventdatastore/{import_id}"
        )],
        import_source: Some(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::ImportSource {
                s3: service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::S3ImportSource {
                    s3_location_uri: format!("s3://bucket-{import_id}/prefix"),
                    s3_bucket_region: HOME_REGION.to_string(),
                    s3_bucket_access_role_arn: "arn:aws:iam::123456789012:role/ImportRole"
                        .to_string(),
                },
            },
        ),
        start_event_time: Some(created - Duration::from_secs(86_400)),
        end_event_time: Some(created),
        status: status.to_string(),
        created_timestamp: created,
        updated_timestamp: created,
        // The `INITIALIZING` -> `IN_PROGRESS` move is due at this instant, which is
        // immaterial here: `StopImport` does not materialise it, and a stop moves
        // either running status to `STOPPED` the same way.
        ingests_at: created,
        statistics: None,
    }
}

/// A member of a reply, as a string.
fn string_member(body: &Value, member: &str, case: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a string {member} in {body:?}"))
        .to_string()
}

/// A timestamp member of a reply, in epoch seconds.
fn seconds_member(body: &Value, member: &str, case: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member} in {body:?}"))
}

/// `StopImport` moves a running import to `STOPPED` and leaves a terminal one alone,
/// across the whole `ImportStatus` enum, and reports the stored record either way.
#[tokio::test]
async fn stop_import_stops_a_running_import_and_leaves_a_terminal_one() {
    let emulator = Emulator::new();

    // (seeded status, the status after the stop, whether the stop moved the import)
    let rows: [(&str, &str, bool); 5] = [
        (IMPORT_STATUS_INITIALIZING, IMPORT_STATUS_STOPPED, true),
        (IMPORT_STATUS_IN_PROGRESS, IMPORT_STATUS_STOPPED, true),
        (IMPORT_STATUS_STOPPED, IMPORT_STATUS_STOPPED, false),
        (IMPORT_STATUS_COMPLETED, IMPORT_STATUS_COMPLETED, false),
        (IMPORT_STATUS_FAILED, IMPORT_STATUS_FAILED, false),
    ];
    // One id per row, each a distinct 36-character `UUID`.
    let ids = [
        "11111111-2222-3333-4444-555555555555",
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        "0123abcd-4567-89ef-0123-456789abcdef",
        "fedcba98-7654-3210-fedc-ba9876543210",
        "deadbeef-dead-beef-dead-beefdeadbeef",
    ];

    for ((seeded, expected, moved), id) in rows.into_iter().zip(ids) {
        let case = format!("StopImport on an import in {seeded}");
        let record = import_record(id, seeded);
        emulator.seed(HOME_REGION, &record);

        let reply = emulator.stop_import(id, HOME_REGION).await;
        let body = reply.expect_ok(&case).clone();

        assert_eq!(
            string_member(&body, "ImportStatus", &case),
            expected,
            "case {case}: expected ImportStatus {expected}, got {body:?}"
        );
        assert_eq!(
            string_member(&body, "ImportId", &case),
            id,
            "case {case}: expected the ImportId from the request, got {body:?}"
        );
        assert_eq!(
            body.get("Destinations"),
            Some(&json!(record.destinations)),
            "case {case}: expected the stored Destinations, got {body:?}"
        );
        assert_eq!(
            body.pointer("/ImportSource/S3/S3LocationUri")
                .and_then(Value::as_str),
            Some(format!("s3://bucket-{id}/prefix").as_str()),
            "case {case}: expected the stored ImportSource, got {body:?}"
        );
        assert_eq!(
            seconds_member(&body, "CreatedTimestamp", &case),
            SEEDED_AT_EPOCH_SECONDS as f64,
            "case {case}: expected the stored CreatedTimestamp, got {body:?}"
        );
        // Nothing was imported, so the import reports no statistics.
        assert_eq!(
            body.get("ImportStatistics"),
            None,
            "case {case}: expected no ImportStatistics, got {body:?}"
        );

        let updated = seconds_member(&body, "UpdatedTimestamp", &case);
        if moved {
            assert!(
                updated > SEEDED_AT_EPOCH_SECONDS as f64,
                "case {case}: a stop that moved the import must stamp UpdatedTimestamp, \
                 got {updated}"
            );
        } else {
            assert_eq!(
                updated, SEEDED_AT_EPOCH_SECONDS as f64,
                "case {case}: a terminal import keeps its UpdatedTimestamp, got {updated}"
            );
        }

        // The write survives the call: the registry holds the new status...
        let persisted = emulator
            .stored(id, HOME_REGION)
            .unwrap_or_else(|| panic!("case {case}: the import must still be in the registry"));
        assert_eq!(
            persisted.status, expected,
            "case {case}: expected the registry to hold {expected}, got {:?}",
            persisted.status
        );
        // ...and a second stop reports exactly the same record, so the stop is
        // idempotent.
        let again = emulator.stop_import(id, HOME_REGION).await;
        let second = again.expect_ok(&case).clone();
        assert_eq!(
            second, body,
            "case {case}: a repeated stop must report the same record, got {second:?}"
        );
    }

    assert_eq!(
        emulator.import_count(),
        ids.len(),
        "a stop must not add or drop records, expected {} imports",
        ids.len()
    );
}

/// An `ImportId` that names no import the caller's Region holds is refused with
/// `ImportNotFoundException`, the status the service answers it with, and a message
/// naming the id from the request.
#[tokio::test]
async fn stop_import_refuses_an_import_id_this_region_does_not_hold() {
    let emulator = Emulator::new();

    // An import that exists, but in another Region: reachable only from there.
    let elsewhere = "cafebabe-cafe-babe-cafe-babecafebabe";
    let seeded = import_record(elsewhere, IMPORT_STATUS_IN_PROGRESS);
    emulator.seed(OTHER_REGION, &seeded);

    // (case, the id to stop, the Region to call from)
    let rows: [(&str, &str, &str); 4] = [
        (
            "an id no Region holds",
            "00000000-0000-0000-0000-000000000001",
            HOME_REGION,
        ),
        (
            "another id no Region holds",
            "7fffffff-ffff-ffff-ffff-ffffffffffff",
            HOME_REGION,
        ),
        (
            "an id held only in another Region",
            elsewhere,
            HOME_REGION,
        ),
        (
            "an id no Region holds, asked for in the other Region",
            "abcdef01-2345-6789-abcd-ef0123456789",
            OTHER_REGION,
        ),
    ];

    for (case, id, region) in rows {
        let reply = emulator.stop_import(id, region).await;
        let body = reply
            .expect_error(case, IMPORT_NOT_FOUND_HTTP_STATUS, "ImportNotFoundException")
            .clone();
        assert_eq!(
            string_member(&body, "Message", case),
            import_not_found_message(id),
            "case {case}: expected a message naming {id}, got {body:?}"
        );
    }

    // A refusal changed nothing: the import in the other Region is untouched, and is
    // still stoppable from there.
    let untouched = emulator
        .stored(elsewhere, OTHER_REGION)
        .expect("the import seeded in the other Region");
    assert_eq!(
        untouched.status, IMPORT_STATUS_IN_PROGRESS,
        "a refused stop must not move an import in another Region, got {:?}",
        untouched.status
    );
    let reply = emulator.stop_import(elsewhere, OTHER_REGION).await;
    let body = reply.expect_ok("StopImport in the import's own Region").clone();
    assert_eq!(
        string_member(&body, "ImportStatus", "StopImport in the import's own Region"),
        IMPORT_STATUS_STOPPED,
        "an import is stoppable from its own Region, got {body:?}"
    );
}

/// An `ImportId` outside the modelled `UUID` constraint is refused at the boundary —
/// at the length bound and one either side of it, and on the character set the
/// pattern allows — and the registry stays as it was.
#[tokio::test]
async fn stop_import_refuses_an_import_id_outside_the_modelled_constraint() {
    let emulator = Emulator::new();

    // The id a seeded import carries, so the accepted rows below prove the boundary
    // let them THROUGH to the handler rather than refusing them.
    let seeded = "0f0f0f0f-1a2b-3c4d-5e6f-7a8b9c0d1e2f";
    emulator.seed(
        HOME_REGION,
        &import_record(seeded, IMPORT_STATUS_IN_PROGRESS),
    );

    // (case, the id, the error code expected, or "" for the handler's own answer)
    let rows: [(&str, String, &str); 7] = [
        ("at the modelled length", seeded.to_string(), ""),
        (
            "at the modelled length, every character a hex digit",
            "0".repeat(IMPORT_ID_LEN),
            "",
        ),
        (
            "one character under the modelled length",
            "1".repeat(IMPORT_ID_LEN - 1),
            "ValidationException",
        ),
        (
            "one character over the modelled length",
            "1".repeat(IMPORT_ID_LEN + 1),
            "ValidationException",
        ),
        ("empty", String::new(), "ValidationException"),
        (
            "a character the pattern excludes",
            format!("{}g", "1".repeat(IMPORT_ID_LEN - 1)),
            "ValidationException",
        ),
        (
            "upper-case hex, which the pattern excludes",
            "AAAAAAAA-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            "ValidationException",
        ),
    ];

    for (case, id, expected) in rows {
        let reply = emulator.stop_import(&id, HOME_REGION).await;
        if expected.is_empty() {
            // Past the boundary, the handler answers from the registry: the seeded id
            // stops, the other one is not found.
            if id == seeded {
                let body = reply.expect_ok(case).clone();
                assert_eq!(
                    string_member(&body, "ImportStatus", case),
                    IMPORT_STATUS_STOPPED,
                    "case {case}: expected the import to stop, got {body:?}"
                );
            } else {
                reply.expect_error(case, IMPORT_NOT_FOUND_HTTP_STATUS, "ImportNotFoundException");
            }
        } else {
            reply.expect_error(case, 400, expected);
        }
    }

    // A refused request persisted nothing: the one seeded import is all there is.
    assert_eq!(
        emulator.import_count(),
        1,
        "a refused StopImport must not touch the registry"
    );
}

/// `StopImport` needs the import registry the shared engine carries: a handler must
/// never build its own store, so a stop made through the registered service is
/// visible to the next reader of that same engine.
#[tokio::test]
async fn stop_import_writes_the_shared_registry() {
    let emulator = Emulator::new();
    let id = "99999999-8888-7777-6666-555544443333";
    emulator.seed(HOME_REGION, &import_record(id, IMPORT_STATUS_INITIALIZING));

    let before = emulator
        .stored(id, HOME_REGION)
        .expect("the seeded import")
        .updated_timestamp;
    emulator
        .stop_import(id, HOME_REGION)
        .await
        .expect_ok("StopImport on the shared engine");
    let after = emulator
        .stored(id, HOME_REGION)
        .expect("the stopped import")
        .updated_timestamp;

    assert_eq!(
        emulator
            .stored(id, HOME_REGION)
            .expect("the stopped import")
            .status,
        IMPORT_STATUS_STOPPED,
        "the stop must be visible in the engine's own registry"
    );
    assert!(
        after > before,
        "the stop must stamp UpdatedTimestamp in the engine's own registry, \
         got {after:?} after {before:?}"
    );
    assert!(
        after > SystemTime::UNIX_EPOCH + Duration::from_secs(SEEDED_AT_EPOCH_SECONDS),
        "the stamped UpdatedTimestamp must be the instant of the call, got {after:?}"
    );
}
