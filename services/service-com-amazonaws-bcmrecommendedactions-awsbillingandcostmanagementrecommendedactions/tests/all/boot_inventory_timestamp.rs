// SPDX-License-Identifier: CC0-1.0

//! What the records the emulator starts with report for
//! `RecommendedAction$lastUpdatedTimeStamp`: the instant the inventory was
//! produced, which is the instant the emulator was built.
//!
//! `ListRecommendedActions` only reports, so no call of it — not the first one —
//! may decide a value a response carries. The suite therefore varies WHEN the
//! first call happens and in WHICH ORDER two emulators are read, and asserts the
//! reported instant tracks construction instead:
//!
//! * a listing issued long after the emulator was built still reports the build
//!   instant, not "just now";
//! * of two emulators built at different instants, the one built FIRST reports the
//!   earlier instant whichever of them is read first;
//! * repeated reads, however far apart, report the same instant.
//!
//! The waits are real ([`std::thread::sleep`]): the workspace pins tokio without
//! the `time` feature, and the instants being compared come from the wall clock.
//! They are a little over a second because the wire form these timestamps are
//! rendered in carries whole seconds.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions as bcm;

/// The awsJson1_0 target this service answers.
const TARGET: &str = "AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name — the routing signal the router reads. The signature is not
/// checked: request authentication is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261029/us-east-1/bcm-recommended-actions/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A wait long enough to be visible in a timestamp rendered to whole seconds,
/// plus a margin so a slow machine still separates the two instants.
const PAST_A_RENDERED_SECOND: Duration = Duration::from_millis(1_400);

/// How far a compared pair of instants may differ and still count as the same
/// one: the wire form carries whole seconds, so an instant can be reported up to
/// a second before the moment it was taken.
const RENDERING_TOLERANCE: f64 = 1.0;

/// One emulator: a shared engine, and a router with this service mounted on it,
/// built exactly the way `core/router` builds it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A fresh emulator, holding the inventory the service computed for this
    /// account.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(engine));
        Self { router }
    }

    /// `ListRecommendedActions` over the whole inventory, reporting the status and
    /// the parsed payload.
    async fn list(&self) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "bcm-recommended-actions.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", TARGET)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(&json!({})).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        });
        (status, payload)
    }
}

/// Seconds since the epoch, as the clock this emulator stamps records with reads
/// them.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is at or after the epoch")
        .as_secs_f64()
}

/// Every record's `lastUpdatedTimeStamp` in `payload`, parsed as seconds since the
/// epoch, keyed by the record's `id`.
///
/// Each value must be present, non-empty and readable as the ISO-8601 instant the
/// member is modelled to carry (`RecommendedAction$lastUpdatedTimeStamp`, a
/// `String` per `smithy_query(query="shape", shape="RecommendedAction")`); a case
/// that cannot read one fails rather than skipping it.
fn stamps(case: &str, payload: &Value) -> Vec<(String, f64)> {
    let records = payload
        .get("recommendedActions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: response carries no recommendedActions: {payload}"));
    assert!(
        !records.is_empty(),
        "case {case:?}: expected the computed inventory, got {payload}"
    );
    records
        .iter()
        .map(|record| {
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a record carries no id: {record}"));
            let text = record
                .get("lastUpdatedTimeStamp")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: record {id:?} carries no lastUpdatedTimeStamp: {record}")
                });
            assert!(
                !text.is_empty(),
                "case {case:?}: record {id:?} carries an empty lastUpdatedTimeStamp: {record}"
            );
            let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: record {id:?} carries a lastUpdatedTimeStamp that is not an \
                     ISO-8601 instant ({error}), got {text:?}"
                )
            });
            (id.to_string(), parsed.as_secs_f64())
        })
        .collect()
}

#[tokio::test]
async fn the_boot_inventory_reports_the_instant_the_emulator_was_built_however_late_it_is_first_read()
{
    // The rule generalises over how long the emulator has been up before anyone
    // lists it: zero, one rendered second, and two.
    let cases: Vec<(&str, Duration)> = vec![
        ("listed immediately", Duration::ZERO),
        ("listed after a second", PAST_A_RENDERED_SECOND),
        ("listed after two seconds", 2 * PAST_A_RENDERED_SECOND),
    ];

    for (case, idle) in cases {
        let built_before = now_seconds();
        let emulator = Emulator::new();
        let built_after = now_seconds();

        std::thread::sleep(idle);

        let read_at = now_seconds();
        let (status, payload) = emulator.list().await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {payload}"
        );

        for (id, stamp) in stamps(case, &payload) {
            assert!(
                stamp >= built_before - RENDERING_TOLERANCE,
                "case {case:?}: record {id:?} must report an instant at or after the emulator was \
                 built ({built_before}), got {stamp}"
            );
            assert!(
                stamp <= built_after + RENDERING_TOLERANCE,
                "case {case:?}: record {id:?} must report the instant the emulator was built \
                 (at or before {built_after}), not the reader's clock ({read_at}), got {stamp}"
            );
        }
    }
}

#[tokio::test]
async fn two_emulators_report_their_own_build_instants_whichever_is_read_first() {
    // Read in both orders: the reported instants must be ordered by when each
    // emulator was BUILT, never by which of them was listed first.
    let cases: Vec<(&str, bool)> = vec![
        ("the older emulator read first", true),
        ("the younger emulator read first", false),
    ];

    for (case, older_first) in cases {
        let older = Emulator::new();
        std::thread::sleep(PAST_A_RENDERED_SECOND);
        let younger = Emulator::new();

        let (older_payload, younger_payload) = if older_first {
            let (older_status, older_payload) = older.list().await;
            let (younger_status, younger_payload) = younger.list().await;
            assert_eq!(
                (older_status, younger_status),
                (200, 200),
                "case {case:?}: expected 200 from both emulators, got {older_status} and \
                 {younger_status}"
            );
            (older_payload, younger_payload)
        } else {
            let (younger_status, younger_payload) = younger.list().await;
            let (older_status, older_payload) = older.list().await;
            assert_eq!(
                (older_status, younger_status),
                (200, 200),
                "case {case:?}: expected 200 from both emulators, got {older_status} and \
                 {younger_status}"
            );
            (older_payload, younger_payload)
        };

        let older_stamps = stamps(case, &older_payload);
        let younger_stamps = stamps(case, &younger_payload);
        assert_eq!(
            older_stamps.len(),
            younger_stamps.len(),
            "case {case:?}: both emulators hold the same inventory, got {older_stamps:?} and \
             {younger_stamps:?}"
        );
        for ((id, older_stamp), (younger_id, younger_stamp)) in
            older_stamps.iter().zip(younger_stamps.iter())
        {
            assert_eq!(
                id, younger_id,
                "case {case:?}: both emulators report the same records in the same order, got \
                 {older_stamps:?} and {younger_stamps:?}"
            );
            assert!(
                older_stamp < younger_stamp,
                "case {case:?}: record {id:?} of the emulator built first must report the earlier \
                 instant, got {older_stamp} against {younger_stamp}"
            );
        }
    }
}

#[tokio::test]
async fn reads_far_apart_report_the_same_instant_for_an_unchanged_record() {
    let emulator = Emulator::new();

    let (status, first) = emulator.list().await;
    assert_eq!(
        status, 200,
        "case \"first read\": expected 200, got {status} with {first}"
    );
    let first = stamps("first read", &first);

    for repeat in 0..2 {
        std::thread::sleep(PAST_A_RENDERED_SECOND);
        let case = format!("read {repeat}");
        let (status, again) = emulator.list().await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {again}"
        );
        let again = stamps(&case, &again);
        assert_eq!(
            again, first,
            "case {case:?}: an unchanged record must keep the instant it was computed with, \
             expected {first:?}, got {again:?}"
        );
    }
}
