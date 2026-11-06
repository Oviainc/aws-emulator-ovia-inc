// SPDX-License-Identifier: CC0-1.0

//! The baseline catalog, as `GetBaseline` serves it over HTTP.
//!
//! Drives `GetBaseline` through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handler's Rust signature.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a `baselineIdentifier` is resolved by the 16-character resource id its ARN ends in,
//!   against this service's baseline catalog; the partition, region and account segments
//!   before that id select nothing, because the same catalog is published in all of them.
//!   The answer is the catalog entry's `name` and `description` plus the ARN THIS endpoint
//!   serves that baseline under — so every catalog entry resolves, through any of those
//!   ARN forms, to the same baseline;
//! * that served ARN names the region the request was signed for and NO account: a
//!   baseline is AWS-managed catalog data no account owns, so its ARN's account segment is
//!   EMPTY (which is why `BaselineArn`'s pattern admits zero digits there), and the ARN it
//!   reports round-trips — feeding it back in resolves the same baseline;
//! * an id the catalog does not hold is `ResourceNotFoundException` (404), including one
//!   differing from a catalog id in a single character and one a caller passed to
//!   `EnableBaseline`: nothing a caller does adds a baseline;
//! * the answer is stable — repeated reads, and reads taken around mutating calls, report
//!   the same baseline.
//!
//! The catalog itself is read from the service
//! (`state::BASELINE_CATALOG`), so a retune of its entries retunes these tests rather
//! than leaving them asserting a stale copy. The shapes come from
//! `smithy_query(query="shape", shape="BaselineArn,GetBaselineOutput")` on this crate's
//! `smithy-model/`: `BaselineArn`'s pattern is
//! `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`, and
//! `GetBaselineOutput` is `arn` (required), `name` (required) and `description`.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogBaseline, BASELINE_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{
    call, call_in_region, emulator, set_up_emulator, Answer, SIGNED_REGION,
};

/// The region these tests sign for unless a case names another, and therefore the region
/// of the ARNs the service mints for them.
const REQUEST_REGION: &str = SIGNED_REGION;

/// The emulator's canonical account id, which the ARNs of the resources an account OWNS
/// carry — and which a baseline ARN, owned by no account, must therefore not carry.
fn emulator_account() -> &'static str {
    engine::emulator_account_id()
}

/// POST `/get-baseline` with `identifier`.
async fn get_baseline(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-baseline",
        serde_json::json!({ "baselineIdentifier": identifier }),
    )
    .await
}

/// One row of the ARN-form table: a case name and a way of writing a baseline ARN around
/// a catalog id. Every form satisfies `BaselineArn`'s pattern, so the front door passes
/// all of them and the handler is what resolves them.
struct ArnForm {
    case: &'static str,
    render: fn(&str) -> String,
}

/// The ARN forms the pattern admits, spanning what the rule generalises over: the
/// partition (`[a-z-]+`), the region segment (`[a-z0-9-]*`, so including empty) and the
/// account segment (`[0-9]{0,12}`, so including empty and a full 12 digits).
fn arn_forms() -> Vec<ArnForm> {
    vec![
        ArnForm {
            case: "the documented form: a region and an empty account",
            render: |id| format!("arn:aws:controltower:us-west-2::baseline/{id}"),
        },
        ArnForm {
            case: "a region other than the request's",
            render: |id| format!("arn:aws:controltower:eu-central-1::baseline/{id}"),
        },
        ArnForm {
            case: "a full 12-digit account segment",
            render: |id| format!("arn:aws:controltower:us-west-2:123456789012:baseline/{id}"),
        },
        ArnForm {
            case: "an empty region segment",
            render: |id| format!("arn:aws:controltower::000000000000:baseline/{id}"),
        },
        ArnForm {
            case: "both optional segments empty",
            render: |id| format!("arn:aws:controltower:::baseline/{id}"),
        },
        ArnForm {
            case: "another partition",
            render: |id| format!("arn:aws-us-gov:controltower:us-gov-west-1::baseline/{id}"),
        },
    ]
}

/// The ARN this endpoint serves `baseline` under when the request is signed for `region`:
/// that region, and an EMPTY account segment, because a baseline is service-managed data
/// no account owns.
///
/// Shared with `crate::list_baselines`, which asserts the same served form over the
/// listing `ListBaselines` reports, so the two suites cannot disagree about it.
pub(crate) fn served_arn_in(region: &str, baseline: &CatalogBaseline) -> String {
    format!("arn:aws:controltower:{region}::baseline/{}", baseline.id)
}

/// [`served_arn_in`] for the region these tests sign for by default.
pub(crate) fn served_arn(baseline: &CatalogBaseline) -> String {
    served_arn_in(REQUEST_REGION, baseline)
}

/// RULE: every baseline in the catalog resolves through `GetBaseline`, by the resource id
/// its ARN ends in and whatever the segments before that id are, to that entry's name and
/// description and to the ARN this endpoint serves it under.
#[tokio::test]
async fn every_catalog_baseline_resolves_through_any_arn_form() {
    let router = emulator();

    for baseline in BASELINE_CATALOG {
        for form in arn_forms() {
            let identifier = (form.render)(baseline.id);
            let case = format!("{} via {}", baseline.name, form.case);

            let answer = get_baseline(&router, &identifier).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 for {identifier:?}, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.string("name"),
                baseline.name,
                "case {case:?}: expected the catalog's name, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("description"),
                baseline.description,
                "case {case:?}: expected the catalog's description, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("arn"),
                served_arn(baseline),
                "case {case:?}: expected the ARN this endpoint serves the baseline \
                 under, got {:?}",
                answer.body
            );
        }
    }
}

/// The regions this rule is checked in, spanning what it generalises over (the request's
/// signed region): the ARN's region segment follows the request, while its account segment
/// is empty in every one of them.
const MINTING_REGIONS: &[&str] = &["us-west-2", "us-east-1", "eu-central-1", "ap-south-1"];

/// RULE: the ARN `GetBaseline` reports for a baseline names the region the request was
/// signed for and NO account — a baseline is AWS-managed catalog data no account owns — and
/// that ARN round-trips: passing it back to `GetBaseline` resolves the same baseline.
#[tokio::test]
async fn the_served_baseline_arn_carries_no_account_and_round_trips() {
    let router = emulator();

    for region in MINTING_REGIONS {
        for baseline in BASELINE_CATALOG {
            let case = format!("{} read from {region}", baseline.name);
            // Asked for through the form the model documents, so the answer's shape is
            // the handler's choice rather than an echo of the request.
            let answer = call_in_region(
                &router,
                region,
                "/get-baseline",
                serde_json::json!({
                    "baselineIdentifier":
                        format!("arn:aws:controltower:{region}::baseline/{}", baseline.id),
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} body {:?}",
                answer.status, answer.body
            );

            let arn = answer.string("arn");
            assert_eq!(
                arn,
                served_arn_in(region, baseline),
                "case {case:?}: expected the account-less ARN of the signed region, got \
                 {arn:?}"
            );

            // `BaselineArn`'s six colon-separated fields: the account field (index 4) is
            // the one the pattern's `[0-9]{0,12}` lets be empty, and it must be.
            let fields: Vec<&str> = arn.split(':').collect();
            assert_eq!(
                fields.len(),
                6,
                "case {case:?}: expected six ARN fields, got {arn:?}"
            );
            assert_eq!(
                (fields[1], fields[2], fields[3], fields[4]),
                ("aws", "controltower", *region, ""),
                "case {case:?}: expected partition/service/region and an EMPTY account \
                 segment, got {arn:?}"
            );
            assert!(
                !arn.contains(emulator_account()),
                "case {case:?}: a baseline is owned by no account, so its ARN must not \
                 carry the emulator account id {:?}, got {arn:?}",
                emulator_account()
            );
            assert_eq!(
                fields[5],
                format!("baseline/{}", baseline.id),
                "case {case:?}: expected the catalog id as the resource segment, got \
                 {arn:?}"
            );

            // The ARN AWS gave the client is an ARN the client may hand back: a read
            // through it reports the very same baseline.
            let round_trip = call_in_region(
                &router,
                region,
                "/get-baseline",
                serde_json::json!({ "baselineIdentifier": arn.clone() }),
            )
            .await;
            assert_eq!(
                (
                    round_trip.status,
                    round_trip.string("arn"),
                    round_trip.string("name"),
                    round_trip.string("description")
                ),
                (
                    200,
                    arn.clone(),
                    baseline.name.to_string(),
                    baseline.description.to_string()
                ),
                "case {case:?}: expected the reported ARN to round-trip to the same \
                 baseline, got {:?}",
                round_trip.body
            );
        }
    }
}

/// RULE (the catalog's own shape): every entry is a distinct, well-formed baseline, so a
/// reported ARN satisfies `BaselineArn` and a reported name identifies one baseline.
#[tokio::test]
async fn the_catalog_entries_are_well_formed_and_distinct() {
    assert!(
        BASELINE_CATALOG.len() >= 2,
        "the catalog must publish more than one baseline, got {}",
        BASELINE_CATALOG.len()
    );

    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        // `BaselineArn`'s resource part: exactly 16 characters of [A-Z0-9].
        assert!(
            baseline.id.len() == 16
                && baseline
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
            "case {:?}: expected a 16-character [A-Z0-9] id, got {:?}",
            baseline.name,
            baseline.id
        );
        assert!(
            !baseline.name.is_empty() && !baseline.description.is_empty(),
            "case {:?}: both required text members must be non-empty, got {baseline:?}",
            baseline.name
        );

        for other in &BASELINE_CATALOG[index + 1..] {
            assert_ne!(
                baseline.id, other.id,
                "cases {:?} and {:?}: two catalog entries must not share an id, got {:?} \
                 twice",
                baseline.name, other.name, baseline.id
            );
            assert_ne!(
                baseline.name, other.name,
                "cases {:?} and {:?}: two catalog entries must not share a name",
                baseline.name, other.name
            );
        }
    }
}

/// RULE: an id the catalog does not hold names no baseline — it is 404
/// `ResourceNotFoundException`, and the refusal reports no baseline members.
#[tokio::test]
async fn an_id_the_catalog_does_not_hold_is_not_found() {
    // A near miss of each catalog id: its last character rotated to another [A-Z0-9]
    // value, so the id is well formed and differs in exactly one character.
    let near_misses: Vec<(String, String)> = BASELINE_CATALOG
        .iter()
        .map(|baseline| {
            let (head, last) = baseline.id.split_at(15);
            let rotated = if last == "Z" { "Y" } else { "Z" };
            (
                format!("{:?} with its last character changed", baseline.name),
                format!("{head}{rotated}"),
            )
        })
        .collect();

    let mut cases: Vec<(String, String)> = vec![
        ("an all-zero id".to_string(), "0000000000000000".to_string()),
        ("an all-Z id".to_string(), "ZZZZZZZZZZZZZZZZ".to_string()),
        (
            "an id no baseline carries".to_string(),
            "9XQV3TLK8MDR5WZA".to_string(),
        ),
    ];
    cases.extend(near_misses);

    let router = emulator();
    for (case, id) in cases {
        for form in arn_forms() {
            let identifier = (form.render)(&id);
            let answer = get_baseline(&router, &identifier).await;
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} via {}: expected 404 ResourceNotFoundException for \
                 {identifier:?}, got {:?} body {:?}",
                form.case,
                (answer.status, answer.error_code.as_deref()),
                answer.body
            );
            assert!(
                answer.body.get("name").is_none() && answer.body.get("arn").is_none(),
                "case {case:?} via {}: a refusal must report no baseline, got {:?}",
                form.case,
                answer.body
            );
        }
    }
}

/// RULE (the stability side of it): the catalog is service-managed, so the answer does not
/// depend on what the account holds — a repeated read reports the same baseline, and a
/// baseline identifier the catalog does not hold is not enabled into it.
#[tokio::test]
async fn the_answer_is_stable_and_no_call_adds_a_baseline() {
    let router = set_up_emulator().await;
    let catalog_baseline = BASELINE_CATALOG
        .first()
        .expect("the catalog publishes at least one baseline");
    let identifier = format!(
        "arn:aws:controltower:us-west-2::baseline/{}",
        catalog_baseline.id
    );

    let first = get_baseline(&router, &identifier).await;
    assert_eq!(
        first.status, 200,
        "the first read: expected 200, got {} body {:?}",
        first.status, first.body
    );

    // An identifier that satisfies `BaselineArn` but names no catalog baseline cannot be
    // enabled either: the enablement is refused, so no call can make the catalog hold it...
    let uncatalogued = "arn:aws:controltower:us-west-2::baseline/QQ1234567890ABCD";
    let enabled = call(
        &router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": uncatalogued,
            "baselineVersion": "4.0",
            "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ct11-1c",
        }),
    )
    .await;
    assert_eq!(
        (enabled.status, enabled.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "enabling an uncatalogued baseline: expected 404 ResourceNotFoundException, got \
         {:?} body {:?}",
        (enabled.status, enabled.error_code.as_deref()),
        enabled.body
    );

    // ...and the catalog still does not hold it.
    let still_absent = get_baseline(&router, uncatalogued).await;
    assert_eq!(
        (still_absent.status, still_absent.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "an uncatalogued baseline after the refused enablement: expected 404 \
         ResourceNotFoundException, got {:?} body {:?}",
        (still_absent.status, still_absent.error_code.as_deref()),
        still_absent.body
    );

    // A call that DOES enable a catalog baseline changes the catalog no more than the
    // refusal did: a read reports the same baseline however often it is taken and whatever
    // the account holds.
    let accepted = call(
        &router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": identifier,
            "baselineVersion": catalog_baseline
                .versions
                .last()
                .expect("a catalog baseline offers at least one version"),
            "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ct11-2c",
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "enabling a catalogued baseline: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );

    let reread = get_baseline(&router, &identifier).await;
    assert_eq!(
        (
            reread.status,
            reread.string("arn"),
            reread.string("name"),
            reread.string("description")
        ),
        (
            200,
            first.string("arn"),
            first.string("name"),
            first.string("description")
        ),
        "a re-read after a mutating call: expected the first read's answer {:?}, got {:?}",
        first.body,
        reread.body
    );
}
