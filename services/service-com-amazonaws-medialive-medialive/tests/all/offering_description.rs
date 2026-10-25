// SPDX-License-Identifier: CC0-1.0

//! `DescribeOffering`: what the offering an id names reports, which ids name no
//! offering at all, and what a read of one leaves behind — end to end over the
//! restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.
//!
//! The expected answers are RE-DERIVED from `ListOfferings`, the other reader of
//! the same catalog (`smithy_query(query="operation",
//! operation="DescribeOffering,ListOfferings")` gives both their bindings), so
//! this suite agrees with the catalog the service reports rather than with the
//! handler's own code. The rows span the catalog — the first and the last
//! offering, and the first and the last of EVERY reservable resource type — so a
//! member that is right for one configuration and wrong for another is caught.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::offering_catalog::every_page;
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The instance URI `DescribeOffering` is bound to
/// (`smithy_query(query="operation", operation="DescribeOffering")`: GET
/// /prod/offerings/{OfferingId}).
fn offering_uri(offering_id: &str) -> String {
    format!("/prod/offerings/{offering_id}")
}

/// Every member `DescribeOfferingResponse` declares, by its wire name
/// (`smithy_query(query="operation", operation="DescribeOffering")`), which is
/// member for member the `Offering` a listing reports.
const OFFERING_MEMBERS: [&str; 11] = [
    "arn",
    "currencyCode",
    "duration",
    "durationUnits",
    "fixedPrice",
    "offeringDescription",
    "offeringId",
    "offeringType",
    "region",
    "resourceSpecification",
    "usagePrice",
];

/// The members an offering reports that are scoped to the region the request was
/// signed for: its `Region` ("AWS region, e.g. 'us-west-2'") and the `Arn` built
/// from it. Every other member names the configuration the id reserves.
const REGION_SCOPED_MEMBERS: [&str; 2] = ["arn", "region"];

/// `DescribeOffering` for `offering_id`, signed for `region`.
async fn describe(router: &MultiServiceRouter, region: &str, offering_id: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", &offering_uri(offering_id), None).await
}

/// One offering's id, as the listing reports it.
fn id_of(offering: &Value) -> String {
    offering["offeringId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a listed offering to carry its id, got {offering}"))
        .to_string()
}

/// The resource type the offering reserves
/// (`ReservationResourceSpecification$ResourceType`).
fn resource_type_of(offering: &Value) -> String {
    offering["resourceSpecification"]["resourceType"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The whole catalog as the listing reports it for `region`, in its one order.
async fn catalog(router: &MultiServiceRouter, region: &str) -> Vec<Value> {
    let offerings = every_page(router, "the whole catalog", region, "", None).await;
    assert!(
        !offerings.is_empty(),
        "expected the catalog to hold offerings to describe, got none"
    );
    offerings
}

/// The rows this suite describes: the first and the last offering of the whole
/// catalog, and the first and the last of every resource type it holds — each
/// paired with the listing's own report of it.
fn rows(offerings: &[Value]) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = Vec::new();
    let mut push = |case: String, offering: &Value| {
        if !rows.iter().any(|(name, _)| *name == case) {
            rows.push((case, offering.clone()));
        }
    };
    push("the catalog's first offering".to_string(), &offerings[0]);
    push(
        "the catalog's last offering".to_string(),
        offerings.last().expect("a non-empty catalog"),
    );

    let mut by_type: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for offering in offerings {
        by_type
            .entry(resource_type_of(offering))
            .or_default()
            .push(offering);
    }
    for (resource_type, group) in by_type {
        push(
            format!("the first {resource_type} offering"),
            group[0],
        );
        push(
            format!("the last {resource_type} offering"),
            group.last().expect("a non-empty group"),
        );
    }
    rows
}

/// Rule: `DescribeOffering` answers the one offering its `OfferingId` names, with
/// every member derived from the configuration that id reserves and the region
/// the request was signed for — so a described offering reports exactly what the
/// catalog's listing reports for the same id.
///
/// The rows differ along the CONFIGURATION the described offering reserves (the
/// first and last offering of the catalog, and of each reservable resource type),
/// because that is what an offering's members are derived from; one described
/// offering is ONE row of them.
#[tokio::test]
async fn describe_offering_reports_the_offering_its_id_names() {
    let router = medialive();
    let offerings = catalog(&router, DEFAULT_REGION).await;

    for (case, listed) in rows(&offerings) {
        let id = id_of(&listed);
        let (status, described) = describe(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeOffering {id} to answer 200, got {status}: \
             {described}"
        );
        assert_eq!(
            described["offeringId"],
            json!(id),
            "case {case:?}: expected the described offering to carry the requested id, got {:?}",
            described["offeringId"]
        );
        for member in OFFERING_MEMBERS {
            assert_eq!(
                described.get(member),
                listed.get(member),
                "case {case:?}: expected {member} to report what the listing reports for \
                 offering {id}, got {:?} against {:?}",
                described.get(member),
                listed.get(member)
            );
            assert!(
                described.get(member).is_some(),
                "case {case:?}: expected the described offering to report {member}, got \
                 {described}"
            );
        }
        assert_eq!(
            described["arn"],
            json!(format!(
                "arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:offering:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the offering id, \
             got {:?}",
            described["arn"]
        );
    }
}

/// Rule: an `OfferingId` the catalog does not hold names no offering, so the read
/// is refused with the operation's modelled `NotFoundException` (404) and reports
/// no offering — while the ids at both ends of the catalog are served.
///
/// The rows differ along HOW the id falls outside the catalog (a value that is no
/// id at all, one below the first, one past the last, one far past it), and both
/// published bounds are asserted beside them, so the refusal is not a blanket
/// rejection.
#[tokio::test]
async fn an_id_the_catalog_does_not_hold_is_not_found() {
    let router = medialive();
    let offerings = catalog(&router, DEFAULT_REGION).await;

    // The bounds of the catalog, read off the listing itself.
    let first: u64 = id_of(&offerings[0]).parse().expect("a numeric offering id");
    let last: u64 = id_of(offerings.last().expect("a non-empty catalog"))
        .parse()
        .expect("a numeric offering id");
    assert!(
        last >= first,
        "expected the catalog's last id {last} to be at or past its first {first}"
    );

    let below = (first - 1).to_string();
    let past = (last + 1).to_string();
    let far_past = (last + 10_000).to_string();
    for (case, id) in [
        ("no id at all", "not-an-offering".to_string()),
        ("an empty-looking id", "0".to_string()),
        ("a negative id", format!("-{first}")),
        ("a fractional id", format!("{first}.5")),
        ("one below the first", below),
        ("one past the last", past),
        ("far past the last", far_past),
    ] {
        let (status, body) = describe(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeOffering {id} to be refused as not found, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for {id}, got {body}"
        );
        for member in OFFERING_MEMBERS {
            assert!(
                body.get(member).is_none(),
                "case {case:?}: expected a refusal to report no offering, got {member} in {body}"
            );
        }
    }

    // Both bounds themselves are served: the catalog's first and last ids.
    for (case, id) in [
        ("the first offering's id", first.to_string()),
        ("the last offering's id", last.to_string()),
    ] {
        let (status, body) = describe(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the bound {id} to be served, got {status}: {body}"
        );
        assert_eq!(
            body["offeringId"],
            json!(id),
            "case {case:?}: expected the offering at the bound, got {:?}",
            body["offeringId"]
        );
    }
}

/// Rule: a describe is a report, so it changes nothing a later caller sees — the
/// same id answers the same offering however many times it is read, and the
/// listing beside it is unchanged — and an id names the same configuration in
/// every region, where only the region-scoped members differ.
///
/// The rows differ along the REGION the read is signed for, because that is the
/// one request-borne input an offering's members depend on.
#[tokio::test]
async fn a_describe_is_a_report_and_an_id_names_one_configuration() {
    let router = medialive();
    let before = catalog(&router, DEFAULT_REGION).await;
    let id = id_of(&before[before.len() / 2]);

    // Repeated reads of one id answer the same offering, and the catalog around
    // it is the same catalog afterwards.
    let (status, home) = describe(&router, DEFAULT_REGION, &id).await;
    assert_eq!(
        status, 200,
        "case \"first read\": expected DescribeOffering {id} to answer 200, got {status}: {home}"
    );
    for attempt in 1..=3 {
        let case = format!("read {attempt}");
        let (status, again) = describe(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated read to answer 200, got {status}: {again}"
        );
        assert_eq!(
            again, home,
            "case {case:?}: expected a repeated read to answer the same offering"
        );
    }
    let after = catalog(&router, DEFAULT_REGION).await;
    assert_eq!(
        after, before,
        "expected reading an offering to leave the catalog a later caller sees unchanged"
    );

    // Another region's endpoint: the same configuration under the same id,
    // reported for that region.
    for region in ["eu-west-1", "ap-southeast-2", DEFAULT_REGION] {
        let case = format!("signed for {region}");
        let (status, away) = describe(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected every region to serve the catalog, got {status}: {away}"
        );
        assert_eq!(
            away["region"],
            json!(region),
            "case {case:?}: expected the offering to be reported for the signed region, got {:?}",
            away["region"]
        );
        assert_eq!(
            away["arn"],
            json!(format!("arn:aws:medialive:{region}:{ACCOUNT}:offering:{id}")),
            "case {case:?}: expected the ARN built from the signed region, got {:?}",
            away["arn"]
        );
        for member in OFFERING_MEMBERS {
            if REGION_SCOPED_MEMBERS.contains(&member) {
                continue;
            }
            if member == "offeringDescription" {
                // The description names the configuration AND the region it
                // reserves it in, so it is compared without that region.
                let description = away[member].as_str().unwrap_or_default();
                assert!(
                    description.contains(region),
                    "case {case:?}: expected the description to name the region, got \
                     {description:?}"
                );
                continue;
            }
            assert_eq!(
                away.get(member),
                home.get(member),
                "case {case:?}: expected {member} to name the configuration the id reserves, \
                 whatever region it is read in, got {:?} against {:?}",
                away.get(member),
                home.get(member)
            );
        }
    }
}
