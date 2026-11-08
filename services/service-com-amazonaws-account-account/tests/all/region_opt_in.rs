// SPDX-License-Identifier: CC0-1.0

//! The AWS Account Region opt-in slice — `GetRegionOptStatus`, `EnableRegion`
//! and `DisableRegion` — end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted store and serialization: the same
//! path a client takes.
//!
//! The rule under test is the one Account Management states for Region opt-in:
//! a Region's `RegionOptStatus` is a property of the ACCOUNT, derived from the
//! Region catalog and from the opt calls made against that account — a Region
//! that is enabled by default always reports `ENABLED_BY_DEFAULT` and is not a
//! valid target of either opt call, and an opt-in Region reports `DISABLED`
//! until `EnableRegion` moves it through `ENABLING` to `ENABLED`, and
//! `DisableRegion` back through `DISABLING` to `DISABLED`. Each case below is
//! one row of that rule, spanning both catalog columns, every one of the five
//! statuses, both opt calls, repeated calls, two accounts and a value that is
//! not a Region code.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Regions the AWS General Reference table "Available Regions"
/// (<https://docs.aws.amazon.com/general/latest/gr/rande-manage.html>) marks
/// "Enabled by default", so no account has to opt in to them. Spelled out here,
/// not imported from the crate, so a wrong catalog in the implementation cannot
/// make this test pass. Four rows from four geographies, none of them cited by
/// the issue this test was written for.
const DEFAULT_ENABLED: [&str; 4] = ["us-east-1", "eu-west-3", "ap-northeast-3", "sa-east-1"];

/// Regions the same table marks "Must be enabled": an account starts with them
/// disabled. Four rows, again from four geographies.
const OPT_IN: [&str; 4] = ["af-south-1", "eu-central-2", "il-central-1", "ap-southeast-4"];

/// Values that are NOT Region codes of this partition: a plausible-looking code
/// that no Region carries, a Region code of another partition (`aws-us-gov`,
/// which this API does not opt into), a bare word, and the longest string
/// `RegionName`'s modelled `@length(min: 1, max: 50)` admits — so the refusal
/// comes from the handler's catalog rule and not from the boundary.
fn not_region_codes() -> Vec<String> {
    vec![
        "us-east-9".to_string(),
        "us-gov-west-1".to_string(),
        "nowhere".to_string(),
        "a".repeat(50),
    ]
}

/// The statuses `RegionOptStatus` admits, from
/// `smithy_query(query="shape", shape="RegionOptStatus")`.
const ENABLED_BY_DEFAULT: &str = "ENABLED_BY_DEFAULT";
const DISABLED: &str = "DISABLED";
const DISABLING: &str = "DISABLING";
const ENABLING: &str = "ENABLING";
const ENABLED: &str = "ENABLED";

/// A second account, named explicitly through the optional `AccountId` member
/// (`^\d{12}$`), used to pin that a Region's status is per account.
const MEMBER_ACCOUNT: &str = "123456789012";

/// An account this emulator holds no record of, so a REPORTING call that names
/// it is refused rather than served.
const UNKNOWN_ACCOUNT: &str = "999999999999";

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// How long a case waits for a transient status to settle before calling the
/// transition broken. Far above the emulator's own transition span, which this
/// test deliberately does not import: a case waits for the OBSERVED status to
/// change rather than for a duration it was told.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

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

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn get_status(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getRegionOptStatus", request).await
}

async fn enable_region(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/enableRegion", request).await
}

async fn disable_region(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/disableRegion", request).await
}

/// The status the emulator reports for `region` on the caller's own account,
/// asserting the call succeeded and echoed the Region code back.
async fn status_of(router: &MultiServiceRouter, region: &str) -> String {
    let answer = get_status(router, json!({ "RegionName": region })).await;
    assert_eq!(
        answer.status, 200,
        "case {region:?}: GetRegionOptStatus must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.member("RegionName"),
        region,
        "case {region:?}: the response must carry the Region code that was passed in, got {:?}",
        answer.body
    );
    answer.member("RegionOptStatus").to_string()
}

/// The status `ListRegions` reports for `region` on the caller's own account —
/// the other reader of the same record.
async fn listed_status(router: &MultiServiceRouter, region: &str) -> String {
    let answer = call(router, "/listRegions", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {region:?}: ListRegions must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("a ListRegions success carries Regions, got {:?}", answer.body))
        .iter()
        .find(|entry| entry["RegionName"].as_str() == Some(region))
        .unwrap_or_else(|| panic!("case {region:?}: the list must carry it, got {:?}", answer.body))
        ["RegionOptStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Poll `region`'s status until it leaves `transient`, and answer what it
/// settled to. Fails the case if it is still transient after
/// [`SETTLE_TIMEOUT`].
async fn settled_status(router: &MultiServiceRouter, region: &str, transient: &str) -> String {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let status = status_of(router, region).await;
        if status != transient {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {region:?}: status is still {transient} after {SETTLE_TIMEOUT:?} — \
             a transition the clock owes must come due on its own"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_default_enabled_region_reports_enabled_by_default_and_is_not_an_opt_target() {
    let router = emulator();

    for region in DEFAULT_ENABLED {
        assert_eq!(
            status_of(&router, region).await,
            ENABLED_BY_DEFAULT,
            "case {region:?}: a Region enabled by default must report {ENABLED_BY_DEFAULT}"
        );

        for (operation, answer) in [
            (
                "EnableRegion",
                enable_region(&router, json!({ "RegionName": region })).await,
            ),
            (
                "DisableRegion",
                disable_region(&router, json!({ "RegionName": region })).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {region:?}/{operation}: a Region enabled by default is not an opt \
                 target, so the call must answer 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {region:?}/{operation}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.member("reason"),
                "fieldValidationFailed",
                "case {region:?}/{operation}: expected reason fieldValidationFailed, got {:?}",
                answer.body
            );
        }

        // The refused calls changed nothing: a later caller sees what the
        // first one saw.
        assert_eq!(
            status_of(&router, region).await,
            ENABLED_BY_DEFAULT,
            "case {region:?}: a refused opt call must leave the status alone"
        );
    }
}

#[tokio::test]
async fn an_opt_in_region_starts_disabled_and_round_trips_through_both_transitions() {
    let router = emulator();

    for region in OPT_IN {
        assert_eq!(
            status_of(&router, region).await,
            DISABLED,
            "case {region:?}: an opt-in Region nobody enabled must report {DISABLED}"
        );

        let enabled = enable_region(&router, json!({ "RegionName": region })).await;
        assert_eq!(
            enabled.status, 200,
            "case {region:?}: EnableRegion must answer 200, got {} with {:?}",
            enabled.status, enabled.body
        );

        // The opt-in is in flight: the transient status is observable, so the
        // transition is never instantaneous.
        assert_eq!(
            status_of(&router, region).await,
            ENABLING,
            "case {region:?}: the status straight after EnableRegion must be {ENABLING}"
        );

        // Opting out of a Region that is still enabling conflicts.
        let conflict = disable_region(&router, json!({ "RegionName": region })).await;
        assert_eq!(
            conflict.status, 409,
            "case {region:?}: DisableRegion while {ENABLING} must answer 409, got {} with {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {region:?}: expected ConflictException, got {:?}",
            conflict.body
        );

        // The clock owes the terminal status, and pays it without being asked
        // for anything but a read.
        assert_eq!(
            settled_status(&router, region, ENABLING).await,
            ENABLED,
            "case {region:?}: an opt-in must settle to {ENABLED}"
        );

        let disabled = disable_region(&router, json!({ "RegionName": region })).await;
        assert_eq!(
            disabled.status, 200,
            "case {region:?}: DisableRegion must answer 200, got {} with {:?}",
            disabled.status, disabled.body
        );
        assert_eq!(
            status_of(&router, region).await,
            DISABLING,
            "case {region:?}: the status straight after DisableRegion must be {DISABLING}"
        );

        // Opting in to a Region that is still disabling conflicts — the mirror
        // of the case above.
        let conflict = enable_region(&router, json!({ "RegionName": region })).await;
        assert_eq!(
            conflict.status, 409,
            "case {region:?}: EnableRegion while {DISABLING} must answer 409, got {} with {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {region:?}: expected ConflictException, got {:?}",
            conflict.body
        );

        assert_eq!(
            settled_status(&router, region, DISABLING).await,
            DISABLED,
            "case {region:?}: an opt-out must settle to {DISABLED}"
        );
    }
}

#[tokio::test]
async fn an_opt_call_that_asks_for_the_status_a_region_already_has_is_a_no_op() {
    let router = emulator();

    for region in OPT_IN {
        // DISABLED is what the Region already is: disabling it again is
        // accepted and leaves it there, with no DISABLING in between.
        let answer = disable_region(&router, json!({ "RegionName": region })).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: disabling an already {DISABLED} Region must answer 200, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            status_of(&router, region).await,
            DISABLED,
            "case {region:?}: disabling an already {DISABLED} Region must leave it {DISABLED}"
        );

        // Two EnableRegion calls in a row: the second does not restart the
        // transition, so the Region still settles to ENABLED.
        for attempt in 1..=2 {
            let answer = enable_region(&router, json!({ "RegionName": region })).await;
            assert_eq!(
                answer.status, 200,
                "case {region:?} attempt {attempt}: EnableRegion must answer 200, \
                 got {} with {:?}",
                answer.status, answer.body
            );
        }
        assert_eq!(
            settled_status(&router, region, ENABLING).await,
            ENABLED,
            "case {region:?}: a repeated opt-in must still settle to {ENABLED}"
        );

        // And once ENABLED, enabling again is accepted and keeps it ENABLED.
        let answer = enable_region(&router, json!({ "RegionName": region })).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: enabling an already {ENABLED} Region must answer 200, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            status_of(&router, region).await,
            ENABLED,
            "case {region:?}: enabling an already {ENABLED} Region must leave it {ENABLED}"
        );
    }
}

#[tokio::test]
async fn a_value_that_is_not_a_region_code_is_refused_by_every_region_operation() {
    let router = emulator();

    for value in not_region_codes() {
        for (operation, uri) in [
            ("GetRegionOptStatus", "/getRegionOptStatus"),
            ("EnableRegion", "/enableRegion"),
            ("DisableRegion", "/disableRegion"),
        ] {
            let answer = call(&router, uri, json!({ "RegionName": value })).await;
            assert_eq!(
                answer.status, 400,
                "case {value:?}/{operation}: a value that is not a Region code must answer \
                 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {value:?}/{operation}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_regions_opt_status_is_per_account() {
    let router = emulator();
    let own = default_account_id();
    let region = OPT_IN[0];

    // Opting in for a member account creates that account's record and moves
    // only ITS status.
    let answer = enable_region(
        &router,
        json!({ "AccountId": MEMBER_ACCOUNT, "RegionName": region }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "EnableRegion for {MEMBER_ACCOUNT} must answer 200, got {} with {:?}",
        answer.status, answer.body
    );

    let member = get_status(
        &router,
        json!({ "AccountId": MEMBER_ACCOUNT, "RegionName": region }),
    )
    .await;
    assert_eq!(
        member.member("RegionOptStatus"),
        ENABLING,
        "case {MEMBER_ACCOUNT}: the account that opted in must report {ENABLING}, got {:?}",
        member.body
    );

    assert_eq!(
        status_of(&router, region).await,
        DISABLED,
        "case {own}: another account's opt-in must leave the caller's own status {DISABLED}"
    );

    // Naming the caller's own id is refused: the caller's own record is reached
    // by the standalone call, which still reports its own status.
    let named = get_status(&router, json!({ "AccountId": own, "RegionName": region })).await;
    assert_eq!(
        (named.status, named.error_code()),
        (403, "AccessDeniedException"),
        "case {own}: naming the caller's own id must be refused, got {} with {:?}",
        named.status,
        named.body
    );
    assert_eq!(
        status_of(&router, region).await,
        DISABLED,
        "case {own}: the refused call must leave the caller's own status {DISABLED}"
    );

    // A reporting call never mints an account record, so an account this
    // emulator knows nothing about is refused.
    let unknown = get_status(
        &router,
        json!({ "AccountId": UNKNOWN_ACCOUNT, "RegionName": region }),
    )
    .await;
    assert_eq!(
        unknown.status, 403,
        "case {UNKNOWN_ACCOUNT}: an account with no record must be refused, got {} with {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "AccessDeniedException",
        "case {UNKNOWN_ACCOUNT}: expected AccessDeniedException, got {:?}",
        unknown.body
    );
}

#[tokio::test]
async fn the_two_readers_of_the_same_record_agree() {
    let router = emulator();
    let region = OPT_IN[1];

    // ListRegions and GetRegionOptStatus report the same per-account record, so
    // they cannot disagree about a Region — before an opt call, with one in
    // flight, and after it has settled.
    for stage in ["before", "in flight", "settled"] {
        if stage == "in flight" {
            let answer = enable_region(&router, json!({ "RegionName": region })).await;
            assert_eq!(
                answer.status, 200,
                "case {stage:?}: EnableRegion must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
        }
        if stage == "settled" {
            assert_eq!(
                settled_status(&router, region, ENABLING).await,
                ENABLED,
                "case {stage:?}: an opt-in must settle to {ENABLED}"
            );
        }

        let listed = listed_status(&router, region).await;
        let read = status_of(&router, region).await;
        assert_eq!(
            listed, read,
            "case {stage:?}: ListRegions reported {listed} for {region:?} where \
             GetRegionOptStatus reported {read}"
        );
        let expected: &[&str] = match stage {
            "before" => &[DISABLED],
            "in flight" => &[ENABLING, ENABLED],
            _ => &[ENABLED],
        };
        assert!(
            expected.contains(&read.as_str()),
            "case {stage:?}: expected one of {expected:?} for {region:?}, got {read}"
        );
    }
}

#[tokio::test]
async fn reading_a_status_does_not_change_what_a_later_caller_sees() {
    let router = emulator();

    // Every catalog column, read twice with nothing in between: the second
    // read must answer exactly what the first did.
    for region in DEFAULT_ENABLED.iter().chain(OPT_IN.iter()) {
        let first = status_of(&router, region).await;
        let second = status_of(&router, region).await;
        assert_eq!(
            first, second,
            "case {region:?}: two reads with no call in between must agree, got {first} then \
             {second}"
        );
    }
}
