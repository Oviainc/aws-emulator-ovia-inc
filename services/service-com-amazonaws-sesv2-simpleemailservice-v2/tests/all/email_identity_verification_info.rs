// SPDX-License-Identifier: CC0-1.0

//! `GetEmailIdentity`'s `VerificationInfo`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization are
//! all exercised, and reads the engine's own state where a response cannot show what
//! was persisted.
//!
//! The rule these cases pin: a domain identity is verified by records published in
//! that domain's own DNS zone, so every read of one reports where the search for
//! those records stands — when it last ran, when it last succeeded, and, while they
//! have not been found, which of the model's two readings of a failed lookup applies:
//! `HOST_NOT_FOUND` ("the queried hostname does not exist") for a name no DNS zone can
//! cover, `TYPE_NOT_FOUND` ("the queried hostname exists but does not have the
//! requested type of DNS record") for a name under a zone that may exist. A name no
//! zone can cover never finishes verifying, however long the caller waits.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, VERIFICATION_PROBE_INTERVAL,
    VERIFICATION_SETTLE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status and the parsed body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The `VerificationInfo` object the read reported, or `Value::Null` when the
    /// member was absent.
    fn info(&self) -> Value {
        self.body
            .get("VerificationInfo")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// `VerificationInfo$ErrorType`, or `""` when the member is absent.
    fn error_type(&self) -> String {
        self.info()
            .get("ErrorType")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A timestamp member of `VerificationInfo`, as the epoch seconds the protocol
    /// renders it with.
    fn stamp(&self, member: &str) -> Option<f64> {
        self.info().get(member).and_then(Value::as_f64)
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can move
/// a stored due instant instead of waiting on the wall clock.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    async fn create(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    async fn get(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// When `identity` was created, as the store recorded it.
    fn created_at(&self, identity: &str) -> SystemTime {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .identities
            .get(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"))
            .created_at
    }

    /// Hold the clock-owed verification transition off, so an assertion about the
    /// state the search reports while the records have not been found cannot race the
    /// span.
    fn hold_verification_pending(&self, identity: &str) {
        self.move_verification_due(
            identity,
            SystemTime::now() + VERIFICATION_SETTLE + Duration::from_secs(3600),
        );
    }

    /// Bring the verification transition's due instant into the past, so the next
    /// read materialises it.
    fn make_verification_due(&self, identity: &str) {
        self.move_verification_due(
            identity,
            SystemTime::now()
                .checked_sub(Duration::from_millis(1))
                .expect("an instant just before now"),
        );
    }

    fn move_verification_due(&self, identity: &str, due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        record.verification_due_at = due_at;
    }

    /// Bring the next search's due instant into the past, so the next read
    /// materialises a fresh one.
    fn make_search_due(&self, identity: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        record.next_check_due_at = SystemTime::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant just before now");
    }
}

/// One domain identity the rule has to hold for: its name, and the reading of a
/// failed lookup that name answers with.
///
/// The rows span both readings and, within each, the shapes a name can take: a
/// second-level name and a subdomain of one, in either case, under a top-level
/// domain that is delegated and under the one that is reserved for names that cannot
/// exist (RFC 6761 section 6.4). `example.com` and its siblings are reserved SECOND-
/// level names, which are delegated and resolve, so they take the delegated reading.
const DOMAIN_CASES: &[(&str, &str)] = &[
    ("example.com", "TYPE_NOT_FOUND"),
    ("mail.example.org", "TYPE_NOT_FOUND"),
    ("Sub.Mixed.Example.NET", "TYPE_NOT_FOUND"),
    ("marketing.example.io", "TYPE_NOT_FOUND"),
    ("nowhere.invalid", "HOST_NOT_FOUND"),
    ("mail.nowhere.invalid", "HOST_NOT_FOUND"),
    ("Mixed.INVALID", "HOST_NOT_FOUND"),
];

#[tokio::test]
async fn a_failed_search_reports_which_reading_the_name_answers() {
    for (identity, expected) in DOMAIN_CASES {
        let ses = Ses::new();
        let created = ses.create(identity).await;
        assert_eq!(
            created.status, 200,
            "case {identity:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );
        ses.hold_verification_pending(identity);

        let read = ses.get(identity).await;
        assert_eq!(
            read.status, 200,
            "case {identity:?}: expected the identity to be readable, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type(),
            *expected,
            "case {identity:?}: expected VerificationInfo.ErrorType {expected}, got {:?}",
            read.info()
        );
        // "An object that contains information about the start of authority (SOA)
        // record associated with the identity" is read out of the zone, and nothing
        // here reads a zone.
        assert_eq!(
            read.info().get("SOARecord"),
            None,
            "case {identity:?}: expected no SOARecord, got {:?}",
            read.info()
        );
        // "The last time a verification attempt was made for this identity": a live
        // instant, after the identity was created.
        let created_at = ses
            .created_at(identity)
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("an instant after the epoch")
            .as_secs_f64();
        let last_checked = read.stamp("LastCheckedTimestamp").unwrap_or_else(|| {
            panic!(
                "case {identity:?}: expected a LastCheckedTimestamp, got {:?}",
                read.info()
            )
        });
        assert!(
            last_checked >= created_at,
            "case {identity:?}: expected the last search to be at or after the create at {created_at}, got {last_checked}"
        );
        // Nothing has succeeded yet, so there is no success to report.
        assert_eq!(
            read.info().get("LastSuccessTimestamp"),
            None,
            "case {identity:?}: expected no LastSuccessTimestamp while the records are not found, got {:?}",
            read.info()
        );
    }
}

#[tokio::test]
async fn a_name_no_zone_can_cover_never_finishes_verifying() {
    // The verification transition the clock owes is materialised — its due instant is
    // brought into the past — and the identity still reports the failed search,
    // because the records it is waiting for cannot be published under that name.
    for (identity, expected) in DOMAIN_CASES {
        if *expected != "HOST_NOT_FOUND" {
            continue;
        }
        let ses = Ses::new();
        ses.create(identity).await;
        ses.make_verification_due(identity);

        let read = ses.get(identity).await;
        assert_eq!(
            (
                read.text("VerificationStatus"),
                read.error_type(),
                read.body
                    .get("VerifiedForSendingStatus")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            ),
            ("PENDING".to_string(), expected.to_string(), false),
            "case {identity:?}: expected the identity to stay unverified with the failed search reported, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_search_that_succeeds_reports_the_success_and_no_error() {
    for (identity, expected) in DOMAIN_CASES {
        if *expected != "TYPE_NOT_FOUND" {
            continue;
        }
        let ses = Ses::new();
        ses.create(identity).await;
        ses.make_verification_due(identity);

        let read = ses.get(identity).await;
        assert_eq!(
            read.text("VerificationStatus"),
            "SUCCESS",
            "case {identity:?}: expected the verification to complete, got {:?}",
            read.body
        );
        assert_eq!(
            read.info().get("ErrorType"),
            None,
            "case {identity:?}: expected no ErrorType once the records are found, got {:?}",
            read.info()
        );
        let success = read.stamp("LastSuccessTimestamp").unwrap_or_else(|| {
            panic!(
                "case {identity:?}: expected a LastSuccessTimestamp, got {:?}",
                read.info()
            )
        });
        let checked = read.stamp("LastCheckedTimestamp").unwrap_or_else(|| {
            panic!(
                "case {identity:?}: expected a LastCheckedTimestamp, got {:?}",
                read.info()
            )
        });
        assert!(
            success <= checked,
            "case {identity:?}: expected the last success at or before the last search, got {success} against {checked}"
        );
    }
}

#[tokio::test]
async fn two_reads_report_one_search_until_the_next_is_due() {
    // The search is a transition the CLOCK owes, so a read reports the one that was
    // owed and never runs an extra search of its own: two reads in the same breath
    // report the same instant, and a read after the next search falls due reports a
    // later one.
    let identity = "cadence.example.com";
    let ses = Ses::new();
    ses.create(identity).await;
    ses.hold_verification_pending(identity);

    let first = ses.get(identity).await;
    let again = ses.get(identity).await;
    assert_eq!(
        first.info(),
        again.info(),
        "case stable: expected two reads to report one search, got {:?} against {:?}",
        first.info(),
        again.info()
    );
    assert!(
        VERIFICATION_PROBE_INTERVAL > Duration::ZERO,
        "case stable: the search cadence must not be zero, got {VERIFICATION_PROBE_INTERVAL:?}"
    );

    ses.make_search_due(identity);
    let later = ses.get(identity).await;
    assert!(
        later.stamp("LastCheckedTimestamp") >= first.stamp("LastCheckedTimestamp"),
        "case due: expected the next search to report a later instant, got {:?} against {:?}",
        later.info(),
        first.info()
    );
    assert_eq!(
        later.error_type(),
        first.error_type(),
        "case due: expected the reading of the failed lookup to be unchanged, got {:?}",
        later.info()
    );
}

#[tokio::test]
async fn an_address_identity_reports_the_search_of_the_domain_that_covers_it() {
    // An email-address identity is verified by the recipient following the link in
    // the verification message, so there is no DNS search to report — unless a domain
    // identity covers it, in which case the search of THAT domain's zone is the one
    // its DKIM records are published in.
    let ses = Ses::new();
    ses.create("solo@uncovered.example.com").await;
    let solo = ses.get("solo@uncovered.example.com").await;
    assert_eq!(
        solo.info(),
        Value::Null,
        "case uncovered: expected no VerificationInfo for an address no domain identity covers, got {:?}",
        solo.info()
    );

    ses.create("covered.invalid").await;
    ses.create("user@mail.covered.invalid").await;
    ses.hold_verification_pending("covered.invalid");
    let parent = ses.get("covered.invalid").await;
    let child = ses.get("user@mail.covered.invalid").await;
    assert_eq!(
        child.info(),
        parent.info(),
        "case covered: expected the address to report the covering domain's search, got {:?} against {:?}",
        child.info(),
        parent.info()
    );
    assert_eq!(
        child.error_type(),
        "HOST_NOT_FOUND",
        "case covered: expected the covering domain's reading of the failed lookup, got {:?}",
        child.info()
    );
}
