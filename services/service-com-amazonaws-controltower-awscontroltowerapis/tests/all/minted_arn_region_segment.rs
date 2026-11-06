// SPDX-License-Identifier: CC0-1.0

//! The region segment of the ARNs this service mints, and whether the identifiers it
//! hands back address the resources they name — end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: the region
//! a request names is the Amazon Web Services Region name its SigV4 credential scope
//! carries, or the service's default Region when that scope carries no Region name
//! (`state::is_region_name`, `state::DEFAULT_REGION`), so the region segment of every ARN
//! this service mints is a Region name — and therefore every identifier a call hands a
//! caller back is one this service's own identifier recognisers accept and resolve,
//! whatever region the request was signed for. The credential scope is caller-controlled
//! text, so the rule is what keeps a minted identifier addressable rather than the
//! caller's choice of scope.
//!
//! The rows vary along what the rule generalises over: the FORM of the signed region —
//! how many hyphen-separated word groups it carries, its case, whether a closing number
//! closes it, where a digit sits — and its LENGTH, on the published bound of the member
//! this service reports a Region name through and one character past it.
//!
//! Every form and bound written out by hand here names its source: the Region name length
//! window is `state::REGION_NAME_LENGTH` (`RegionName`, length 1..50, from
//! `smithy_query(query="shape", shape="Region,RegionName")`), the served baseline ARN form
//! is `BaselineArn`'s pattern
//! `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$` (reused
//! through [`crate::baseline_catalog::served_arn_in`] rather than copied), and the
//! landing zone version, baseline catalog, control catalog identifiers, targets and
//! enablement rows come from the service's own constants and the sibling suites' tables.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    is_baseline_arn, is_landing_zone_arn, is_region_name, BASELINE_CATALOG,
    CATALOG_LANDING_ZONE_VERSION, DEFAULT_REGION, REGION_NAME_LENGTH,
};

use crate::baseline_catalog::served_arn_in;
use crate::baseline_operations::{enablements, Enablement};
use crate::enabled_control_lifecycle::{control, entries, ou};
use crate::landing_zone_lifecycle::{
    call_in_region, emulator, past_the_settle_span, set_up_emulator, simple_manifest, Answer,
};

/// One row of the table: the region a request is signed for, and the region the ARNs that
/// request mints must therefore carry.
struct SigningRegion {
    case: String,
    signed_for: String,
    minted_in: String,
}

/// A value of exactly `length` characters carrying a Region name's form: two word groups
/// closed by the Region's number. Used for the rows that sit on `RegionName`'s published
/// length bound and one character past it.
fn region_shaped_value_of_length(length: usize) -> String {
    let filler = length - "us-".len() - "-1".len();
    format!("us-{}-1", "a".repeat(filler))
}

/// The table: a signed region and the region the service must mint in for it.
///
/// The expected region is COMPUTED from the service's own predicate rather than written
/// out per row, so a retune of what counts as a Region name retunes the table instead of
/// leaving it asserting a stale split.
fn signing_regions() -> Vec<SigningRegion> {
    let signed: Vec<(&'static str, String)> = vec![
        ("a commercial Region name", "us-west-2".to_string()),
        (
            "a Region name in another geography",
            "eu-central-1".to_string(),
        ),
        (
            "a GovCloud Region name, three word groups",
            "us-gov-east-1".to_string(),
        ),
        (
            "an isolated Region name, four word groups",
            "eu-isoe-west-1".to_string(),
        ),
        (
            "a Region name exactly on RegionName's published maximum",
            region_shaped_value_of_length(*REGION_NAME_LENGTH.end()),
        ),
        ("upper case", "US-WEST-2".to_string()),
        ("no word groups at all", "uswest2".to_string()),
        ("no closing number", "us-west".to_string()),
        ("the number spelled out", "us-west-two".to_string()),
        ("only a geography and a number", "us-2".to_string()),
        ("a digit inside a word group", "us-we5t-2".to_string()),
        (
            "one character past RegionName's published maximum",
            region_shaped_value_of_length(*REGION_NAME_LENGTH.end() + 1),
        ),
    ];

    let rows: Vec<SigningRegion> = signed
        .into_iter()
        .map(|(case, signed_for)| {
            // A credential scope is carried in an HTTP header and the same value names the
            // endpoint's authority, so every row must be a value both admit.
            assert!(
                !signed_for.is_empty()
                    && signed_for
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
                "case {case:?}: a signed region must be a value a header and an authority \
                 admit, got {signed_for:?}"
            );
            let minted_in = if is_region_name(&signed_for) {
                signed_for.clone()
            } else {
                DEFAULT_REGION.to_string()
            };
            SigningRegion {
                case: format!("signed for {case} ({signed_for:?})"),
                signed_for,
                minted_in,
            }
        })
        .collect();

    // The table must really span both sides of the rule, so it cannot drift into testing
    // only the regions that pass straight through.
    let names = rows
        .iter()
        .filter(|row| is_region_name(&row.signed_for))
        .count();
    assert!(
        names >= 3 && rows.len() - names >= 3,
        "the table must span both Region names and values that are none, got {names} of \
         {} rows",
        rows.len()
    );
    rows
}

/// The region field of `arn` — field 3 of the six an ARN carries — asserting the value
/// really is an ARN of six fields.
fn arn_region(case: &str, arn: &str) -> String {
    let fields: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected an ARN of six colon-separated fields, got {arn:?}"
    );
    fields[3].to_string()
}

/// Assert `arn` was minted in `row.minted_in`.
fn minted_in_the_requests_region(row: &SigningRegion, what: &str, arn: &str) {
    let region = arn_region(&row.case, arn);
    assert_eq!(
        region, row.minted_in,
        "case {:?}: expected the {what} ARN to carry the region {:?}, got {region:?} in \
         {arn:?}",
        row.case, row.minted_in
    );
    assert!(
        is_region_name(&region),
        "case {:?}: expected the {what} ARN's region segment to be a Region name, got \
         {region:?} in {arn:?}",
        row.case
    );
}

/// POST `path` signed for `row`, asserting a 200 and handing back the answer.
async fn accepted(
    router: &MultiServiceRouter,
    row: &SigningRegion,
    path: &str,
    body: serde_json::Value,
) -> Answer {
    let answer = call_in_region(router, &row.signed_for, path, body).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected {path} to answer 200, got {} body {:?}",
        row.case, answer.status, answer.body
    );
    answer
}

/// The enablement row the baseline cases drive `EnableBaseline` with: one the sibling
/// suite's table already establishes is accepted, so these rows turn on the signed region
/// alone.
fn enablement() -> Enablement {
    enablements()
        .into_iter()
        .next()
        .expect("the enablement table has at least one row")
}

/// RULE: the ARN `CreateLandingZone` hands back is minted in the request's region — a
/// Region name whatever the credential scope carried — so it is a landing zone ARN this
/// service recognises, it is the ARN `ListLandingZones` reports, and every operation that
/// takes a `landingZoneIdentifier` accepts it: the landing zone can be read, updated,
/// reset and finally deleted, which frees the account's one landing zone slot again.
///
/// The rows share no state — an account holds one landing zone, so each gets its own
/// emulator — and they run in PHASES, so the clock span each asynchronous operation needs
/// is waited out once for the whole table rather than once per row.
#[tokio::test]
async fn a_minted_landing_zone_arn_is_addressable_whatever_region_was_signed_for() {
    let mut deployed: Vec<(SigningRegion, MultiServiceRouter, String)> = Vec::new();

    for row in signing_regions() {
        let router = emulator();
        let created = accepted(
            &router,
            &row,
            "/create-landingzone",
            serde_json::json!({
                "version": CATALOG_LANDING_ZONE_VERSION,
                "manifest": simple_manifest(),
            }),
        )
        .await;
        let arn = created.string("arn");
        minted_in_the_requests_region(&row, "landing zone", &arn);
        assert!(
            is_landing_zone_arn(&arn),
            "case {:?}: expected the minted ARN to be one this service recognises as a \
             landing zone ARN, got {arn:?}",
            row.case
        );

        // The identifier a caller is handed is the one the listing reports.
        let listed = accepted(&router, &row, "/list-landingzones", serde_json::json!({})).await;
        assert_eq!(
            listed.get("landingZones"),
            &serde_json::json!([{ "arn": arn }]),
            "case {:?}: expected the listing to report the minted ARN, got {:?}",
            row.case,
            listed.get("landingZones")
        );

        // And the one the read resolves, straight away.
        let read = accepted(
            &router,
            &row,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn.clone() }),
        )
        .await;
        assert_eq!(
            read.get("landingZone").get("arn"),
            Some(&serde_json::json!(arn)),
            "case {:?}: expected the landing zone to report its own ARN, got {:?}",
            row.case,
            read.get("landingZone")
        );

        deployed.push((row, router, arn));
    }

    // Each mutation below needs the previous one's operation to be due, so every row waits
    // the same span at the same time.
    tokio::time::sleep(past_the_settle_span()).await;
    for (row, router, arn) in &deployed {
        accepted(
            router,
            row,
            "/update-landingzone",
            serde_json::json!({
                "landingZoneIdentifier": arn,
                "version": CATALOG_LANDING_ZONE_VERSION,
                "manifest": simple_manifest(),
            }),
        )
        .await;
    }

    tokio::time::sleep(past_the_settle_span()).await;
    for (row, router, arn) in &deployed {
        accepted(
            router,
            row,
            "/reset-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
    }

    tokio::time::sleep(past_the_settle_span()).await;
    for (row, router, arn) in &deployed {
        accepted(
            router,
            row,
            "/delete-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
    }

    // The delete really tore the landing zone down: once it is due, the account holds none
    // and its one slot takes a new one — so nothing a caller cannot address is left behind.
    tokio::time::sleep(past_the_settle_span()).await;
    for (row, router, _) in &deployed {
        let listed = accepted(router, row, "/list-landingzones", serde_json::json!({})).await;
        let remaining = entries(listed.get("landingZones"));
        assert!(
            remaining.is_empty(),
            "case {:?}: expected the deleted landing zone to be gone, got {remaining:?}",
            row.case
        );
        accepted(
            router,
            row,
            "/create-landingzone",
            serde_json::json!({
                "version": CATALOG_LANDING_ZONE_VERSION,
                "manifest": simple_manifest(),
            }),
        )
        .await;
    }
}

/// RULE (the same rule where the model states it): the ARN `GetBaseline` serves a baseline
/// under is minted in the request's region, so it satisfies `BaselineArn` — the one Control
/// Tower ARN the model constrains precisely — and round-trips: it resolves back through
/// `GetBaseline` and names a baseline `EnableBaseline` accepts, whatever region the request
/// was signed for.
#[tokio::test]
async fn a_served_baseline_arn_round_trips_whatever_region_was_signed_for() {
    for row in signing_regions() {
        let router = set_up_emulator().await;

        for baseline in BASELINE_CATALOG {
            let case = format!("{}: {}", row.case, baseline.name);
            // Asked for through the documented form with a region of its own, so the
            // answer's region is the service's choice rather than an echo of the request.
            let served = accepted(
                &router,
                &row,
                "/get-baseline",
                serde_json::json!({
                    "baselineIdentifier": served_arn_in("us-west-2", baseline),
                }),
            )
            .await;
            let arn = served.string("arn");
            minted_in_the_requests_region(&row, "served baseline", &arn);
            assert_eq!(
                arn,
                served_arn_in(&row.minted_in, baseline),
                "case {case:?}: expected the account-less ARN of the request's region, got \
                 {arn:?}"
            );
            assert!(
                is_baseline_arn(&arn),
                "case {case:?}: expected the served ARN to be one this service recognises \
                 as a baseline ARN, got {arn:?}"
            );

            // The front door enforces `BaselineArn`'s pattern before the handler runs, so
            // a round-trip that answers 200 is also what proves the served ARN satisfies
            // it.
            let round_trip = accepted(
                &router,
                &row,
                "/get-baseline",
                serde_json::json!({ "baselineIdentifier": arn.clone() }),
            )
            .await;
            assert_eq!(
                (round_trip.string("arn"), round_trip.string("name")),
                (arn.clone(), baseline.name.to_string()),
                "case {case:?}: expected the served ARN to round-trip to the same baseline, \
                 got {:?}",
                round_trip.body
            );
        }

        // And it is accepted where a baseline ARN is a resource reference rather than a
        // read's subject.
        let row_enablement = enablement();
        let served = accepted(
            &router,
            &row,
            "/get-baseline",
            serde_json::json!({ "baselineIdentifier": row_enablement.baseline_identifier }),
        )
        .await;
        accepted(
            &router,
            &row,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": served.string("arn"),
                "baselineVersion": row_enablement.baseline_version,
                "targetIdentifier": row_enablement.target_identifier,
            }),
        )
        .await;
    }
}

/// RULE (the same rule on the enablement resources): the ARN `EnableControl` and
/// `EnableBaseline` mint is minted in the request's region too, and resolves through the
/// enablement's own reporting call, whatever region the request was signed for.
#[tokio::test]
async fn a_minted_enablement_arn_resolves_whatever_region_was_signed_for() {
    for row in signing_regions() {
        let router = set_up_emulator().await;

        let enabled_control = accepted(
            &router,
            &row,
            "/enable-control",
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou("6060"),
            }),
        )
        .await;
        let control_arn = enabled_control.string("arn");
        minted_in_the_requests_region(&row, "enabled control", &control_arn);
        let control_details = accepted(
            &router,
            &row,
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": control_arn.clone() }),
        )
        .await;
        assert_eq!(
            control_details.get("enabledControlDetails").get("arn"),
            Some(&serde_json::json!(control_arn)),
            "case {:?}: expected the enabled control to report its own ARN, got {:?}",
            row.case,
            control_details.get("enabledControlDetails")
        );

        let row_enablement = enablement();
        let enabled_baseline = accepted(
            &router,
            &row,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": row_enablement.baseline_identifier,
                "baselineVersion": row_enablement.baseline_version,
                "targetIdentifier": row_enablement.target_identifier,
            }),
        )
        .await;
        let baseline_arn = enabled_baseline.string("arn");
        minted_in_the_requests_region(&row, "enabled baseline", &baseline_arn);
        let baseline_details = accepted(
            &router,
            &row,
            "/get-enabled-baseline",
            serde_json::json!({ "enabledBaselineIdentifier": baseline_arn.clone() }),
        )
        .await;
        assert_eq!(
            baseline_details.get("enabledBaselineDetails").get("arn"),
            Some(&serde_json::json!(baseline_arn)),
            "case {:?}: expected the enablement to report its own ARN, got {:?}",
            row.case,
            baseline_details.get("enabledBaselineDetails")
        );
    }
}
