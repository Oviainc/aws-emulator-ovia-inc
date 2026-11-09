// SPDX-License-Identifier: CC0-1.0

//! DescribeOffering, end to end over HTTP: `GET /v1/offerings/{OfferingArn}`.
//!
//! The subject is the rule DescribeOffering follows: the offering the response carries must
//! be the entry of the catalogue this service publishes for the Region the request was
//! signed for that the `OfferingArn` names — the SAME projection ListOfferings reports, so
//! a described offering cannot differ from a listed one — and an ARN the catalogue publishes
//! no offering under must be refused with the modelled `NotFoundException`.
//!
//! Nothing in the response is fixed in the handler, so the cases below vary what the rule
//! generalises over: which published entry is addressed (every tier of the bandwidth ladder,
//! read from the listing rather than written down here), the Region its ARN names, and the
//! ways an identifier can resolve no offering. No request creates an offering — the
//! catalogue is AWS's own, assigned the ARN by MediaConnect ("the Amazon Resource Name
//! (ARN) that MediaConnect assigns to the offering",
//! `smithy_query(query="shape",shape="Offering",member="OfferingArn")`) — so these tests
//! also assert that describing one changes nothing.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! door uses, so what these tests assert is what a MediaConnect client sees: routing,
//! restJson1 deserialization, constraint validation, handler, serialization.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::offering_listing::{assert_minted_offering_arn, list_offerings, listed, listed_arns};
use crate::reservation_listing::{
    seed_reservation, ReservationCase, CURRENCY_CODE, DURATION_UNITS, ONE_MONTH, PRICE_UNITS,
    RESOURCE_TYPE,
};

async fn describe_offering(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/offerings/{arn}"), None)
        .await
}

/// The same request, signed for `region` instead of [`REGION`], so a case can address the
/// catalogue of another Region.
async fn describe_offering_signed_for(
    emulator: &Emulator,
    region: &str,
    arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send_signed_for(region, "GET", &format!("/v1/offerings/{arn}"), None)
        .await
}

/// The whole published catalogue, as the listing reports it for [`REGION`] — the set of
/// entries DescribeOffering must answer for, taken from the service rather than restated
/// here, so a change to the ladder is covered by these cases without editing them.
async fn published_catalogue(emulator: &Emulator) -> Vec<Value> {
    let (status, code, body) = list_offerings(emulator, "?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the catalogue must be listable, got {status} {code:?}: {body}"
    );
    let offerings = listed(&body);
    assert_eq!(
        offerings.len(),
        mc::OFFERING_CATALOGUE.len(),
        "the listing must carry the whole published catalogue, got {body}"
    );
    offerings
}

/// `Offering$PricePerUnit` as an integer count of thousandths of a dollar, which is what a
/// rate quoted to three decimal places is — and an assertion on the member's form while it
/// parses it.
fn price_in_thousandths(case: &str, price_per_unit: &str) -> i64 {
    let (dollars, fraction) = price_per_unit.split_once('.').unwrap_or_else(|| {
        panic!("case {case:?}: a price is a decimal string, got {price_per_unit:?}")
    });
    assert_eq!(
        fraction.len(),
        3,
        "case {case:?}: a price is quoted to three decimal places, got {price_per_unit:?}"
    );
    let parsed: Result<(i64, i64), _> = dollars
        .parse::<i64>()
        .and_then(|dollars| fraction.parse::<i64>().map(|fraction| (dollars, fraction)));
    let (dollars, fraction) = parsed.unwrap_or_else(|_| {
        panic!("case {case:?}: a price holds only digits and one point, got {price_per_unit:?}")
    });
    dollars * 1_000 + fraction
}

/// The `offering` member of a DescribeOffering response.
fn described(body: &Value) -> Value {
    body["offering"].clone()
}

/// DescribeOffering answers, for every tier the catalogue publishes, exactly the entry the
/// ARN names: the described offering is the listed one member for member, its terms are the
/// values the modelled enums carry, and its description states its own bandwidth and price.
#[tokio::test]
async fn every_published_offering_is_described_as_it_is_listed() {
    let emulator = Emulator::new();
    let catalogue = published_catalogue(&emulator).await;

    for listed_offering in &catalogue {
        let bitrate = listed_offering["resourceSpecification"]["reservedBitrate"]
            .as_i64()
            .unwrap_or_default();
        let case = format!("the {bitrate} Mbps offering");
        let case = case.as_str();
        let arn = listed_offering["offeringArn"].as_str().unwrap_or_default();
        assert_minted_offering_arn(case, arn, REGION);

        let (status, code, body) = describe_offering(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: expected 200, got {status} {code:?}: {body}"
        );
        let offering = described(&body);
        assert_eq!(
            &offering, listed_offering,
            "case {case:?}: the described offering IS the listed one, member for member"
        );
        assert_eq!(
            offering["offeringArn"].as_str(),
            Some(arn),
            "case {case:?}: the described offering is the one the ARN names, got {body}"
        );
        assert_eq!(
            (
                offering["durationUnits"].as_str(),
                offering["priceUnits"].as_str(),
                offering["currencyCode"].as_str(),
                offering["resourceSpecification"]["resourceType"].as_str(),
            ),
            (
                Some(DURATION_UNITS),
                Some(PRICE_UNITS),
                Some(CURRENCY_CODE),
                Some(RESOURCE_TYPE),
            ),
            "case {case:?}: the billing terms are the only values the modelled enums carry, \
             got {body}"
        );
        let duration = offering["duration"].as_i64().unwrap_or_default();
        assert!(
            duration > 0,
            "case {case:?}: an offering runs for a positive number of months, got {body}"
        );
        assert!(
            bitrate > 0,
            "case {case:?}: an offering reserves a positive amount of bandwidth, got {body}"
        );
        let price_per_unit = offering["pricePerUnit"].as_str().unwrap_or_default();
        let price = price_in_thousandths(case, price_per_unit);
        assert!(
            price > 0,
            "case {case:?}: a reservation is priced above nothing, got {body}"
        );
        // The description is the wording this service publishes an offering under, with
        // the bandwidth and the price of THIS entry read back out of its own members.
        assert_eq!(
            offering["offeringDescription"].as_str(),
            Some(
                format!("Reserved outbound bandwidth of {bitrate}Mbps at ${price_per_unit}/hour")
                    .as_str()
            ),
            "case {case:?}: the description states this entry's own terms, got {body}"
        );
    }

    // Every entry was addressed by a DIFFERENT ARN: the catalogue's ARNs are distinct, so
    // these cases are as many offerings as the ladder has tiers.
    let mut arns = listed_arns(&serde_json::json!({ "offerings": catalogue }));
    let described_count = arns.len();
    arns.sort();
    arns.dedup();
    assert_eq!(
        arns.len(),
        described_count,
        "each described offering has its own ARN, got {arns:?}"
    );
}

/// An offering's Region is part of its identity: the catalogue is published in the Region
/// the request is signed for, so the ARN listed in one Region is described in that Region —
/// and resolves no offering in another.
#[tokio::test]
async fn an_offering_is_published_per_region() {
    let emulator = Emulator::new();
    // Rows: the Region every other suite signs for, one in another partition's geography,
    // and one with a digit in its name. The entries are the same ladder in each.
    let regions = [REGION, "eu-west-2", "ap-southeast-4"];

    let mut first_arn_per_region: Vec<String> = Vec::new();
    for region in regions {
        let (status, code, listing) = emulator
            .send_signed_for(region, "GET", "/v1/offerings?maxResults=1000", None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "region {region:?}: listing expected 200, got {status} {code:?}: {listing}"
        );
        let offerings = listed(&listing);
        for listed_offering in &offerings {
            let arn = listed_offering["offeringArn"].as_str().unwrap_or_default();
            let (status, code, body) = describe_offering_signed_for(&emulator, region, arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "region {region:?}: expected 200 for {arn:?}, got {status} {code:?}: {body}"
            );
            assert_eq!(
                &described(&body),
                listed_offering,
                "region {region:?}: the described offering is the one this Region lists"
            );
        }
        first_arn_per_region.push(
            offerings
                .first()
                .map(|offering| {
                    offering["offeringArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .unwrap_or_default(),
        );
    }

    for (index, region) in regions.iter().enumerate() {
        for (other_index, other) in regions.iter().enumerate() {
            if index == other_index {
                continue;
            }
            let arn = &first_arn_per_region[other_index];
            let (status, code, body) = describe_offering_signed_for(&emulator, region, arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "signed for {region:?}: the ARN of {other:?}'s catalogue resolves no offering, \
                 got {status} {code:?}: {body}"
            );
        }
    }
}

/// A read does not change what a later caller sees: describing every published offering
/// twice answers the same entry both times, and leaves the catalogue and the account's
/// reservations as they were.
#[tokio::test]
async fn describing_an_offering_changes_nothing() {
    let emulator = Emulator::new();
    let (_, _, before) = list_offerings(&emulator, "?maxResults=1000").await;

    for listed_offering in &listed(&before) {
        let arn = listed_offering["offeringArn"].as_str().unwrap_or_default();
        let (first_status, _, first) = describe_offering(&emulator, arn).await;
        let (second_status, _, second) = describe_offering(&emulator, arn).await;
        assert_eq!(
            (first_status, second_status),
            (200, 200),
            "case {arn:?}: both reads expected 200, got {first_status} and {second_status}"
        );
        assert_eq!(
            first, second,
            "case {arn:?}: describing twice reports the same offering"
        );
    }

    let (_, _, after) = list_offerings(&emulator, "?maxResults=1000").await;
    assert_eq!(
        after, before,
        "describing offerings leaves the published catalogue as it was, got {after}"
    );
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    assert!(
        store.reservations.is_empty(),
        "reading the catalogue buys nothing: the account holds no reservation, got {:?}",
        store.reservations.keys().collect::<Vec<_>>()
    );
}

/// Eligibility scopes what the account is OFFERED, not what a catalogue entry IS: "if you
/// have an active reservation ... your account isn't eligible for other offerings" is
/// documented on ListOfferings (`smithy_query(query="operation",operation="ListOfferings")`),
/// so an account holding a running reservation is listed no offerings while the entry its
/// ARN names is still described.
#[tokio::test]
async fn an_offering_is_described_while_the_account_is_offered_none() {
    let emulator = Emulator::new();
    let catalogue = published_catalogue(&emulator).await;
    let arn = catalogue
        .first()
        .and_then(|offering| offering["offeringArn"].as_str())
        .unwrap_or_default()
        .to_string();

    // A reservation that has started and has not expired — what makes the account
    // ineligible — seeded the way a purchase stores one (PurchaseOffering is still a
    // scaffold).
    seed_reservation(
        &emulator,
        &ReservationCase {
            case: "a reservation the account is holding",
            name: "res-holding",
            duration: 12,
            price_per_unit: "0.1000",
            reserved_bitrate: Some(100),
            start_before_now: Duration::from_secs(60),
            end_from_now: Ok(ONE_MONTH),
            stored_state: mc::RESERVATION_STATE_ACTIVE,
            pending: None,
            expected_state: mc::RESERVATION_STATE_ACTIVE,
        },
        SystemTime::now(),
    );

    let (status, _, listing) = list_offerings(&emulator, "?maxResults=1000").await;
    assert_eq!(
        (status, listed(&listing).len()),
        (200, 0),
        "an account holding a running reservation is offered nothing, got {listing}"
    );

    let (status, code, body) = describe_offering(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the offering the ARN names is still described, got {status} {code:?}: {body}"
    );
    assert_eq!(
        described(&body)["offeringArn"].as_str(),
        Some(arn.as_str()),
        "the described offering is the one the ARN names, got {body}"
    );
}

/// One DescribeOffering request that resolves no offering.
struct MissingCase {
    case: &'static str,
    /// The `OfferingArn` label the request carries.
    arn: String,
}

/// DescribeOffering refuses an ARN the catalogue publishes no offering under with the
/// modelled `NotFoundException` (404) — never with a 501, never with a success and never
/// with another offering. The rows are the ways an identifier can resolve nothing: an
/// amount of bandwidth and a term the ladder does not offer, an offering id of another
/// form, an ARN carrying an account-id field the published form does not have, another
/// service's ARN, another resource type of this service, an ARN with a segment appended or
/// a character changed, and a string that is no ARN at all.
#[tokio::test]
async fn an_unpublished_offering_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let catalogue = published_catalogue(&emulator).await;
    let published_arn = catalogue
        .first()
        .and_then(|offering| offering["offeringArn"].as_str())
        .unwrap_or_default()
        .to_string();
    let published_bitrate = catalogue
        .first()
        .and_then(|offering| offering["resourceSpecification"]["reservedBitrate"].as_i64())
        .unwrap_or_default();
    let published_duration = catalogue
        .first()
        .and_then(|offering| offering["duration"].as_i64())
        .unwrap_or_default();
    // An amount the ladder does not hold: one Mbps below its smallest tier.
    let unoffered_bitrate = i32::try_from(published_bitrate - 1).unwrap_or_default();
    let offered_duration = i32::try_from(published_duration).unwrap_or_default();

    let offering_arn_of = |fragment: String| mc::offering_arn(REGION, &fragment);
    let missing = vec![
        MissingCase {
            case: "an amount of bandwidth the catalogue does not offer",
            arn: offering_arn_of(mc::offering_resource_fragment(
                unoffered_bitrate,
                offered_duration,
            )),
        },
        MissingCase {
            case: "a term the catalogue does not offer",
            arn: offering_arn_of(mc::offering_resource_fragment(
                i32::try_from(published_bitrate).unwrap_or_default(),
                offered_duration + 1,
            )),
        },
        MissingCase {
            case: "an offering id of another form",
            arn: offering_arn_of("offering:1-deadbeef".to_string()),
        },
        MissingCase {
            case: "an offering ARN carrying an account-id field",
            arn: published_arn.replace(
                &format!(":{}:", mc::SERVICE_NAMESPACE),
                &format!(
                    ":{}:{}:",
                    mc::SERVICE_NAMESPACE,
                    engine::emulator_account_id()
                ),
            ),
        },
        MissingCase {
            case: "another service's ARN",
            arn: published_arn.replace(":mediaconnect:", ":medialive:"),
        },
        MissingCase {
            case: "another resource type of this service",
            arn: published_arn.replace(":offering:", ":flow:"),
        },
        MissingCase {
            case: "the published ARN with a segment appended",
            arn: format!("{published_arn}:extra"),
        },
        MissingCase {
            case: "the published ARN with one character changed",
            arn: format!("{published_arn}x"),
        },
        MissingCase {
            case: "an identifier that is no ARN at all",
            arn: "not-an-arn".to_string(),
        },
    ];

    for MissingCase { case, arn } in &missing {
        assert_ne!(
            arn, &published_arn,
            "case {case:?}: the row must address something other than the published offering"
        );
        let (status, code, body) = describe_offering(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body}"
        );
        assert!(
            body["offering"].is_null(),
            "case {case:?}: a refusal carries no offering, got {body}"
        );
    }

    // The published offering is still readable: a refused read did not retire it.
    let (status, code, body) = describe_offering(&emulator, &published_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the published offering is still described, got {status} {code:?}: {body}"
    );
}
