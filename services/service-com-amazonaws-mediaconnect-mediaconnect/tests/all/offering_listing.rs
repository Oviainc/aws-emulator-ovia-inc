// SPDX-License-Identifier: CC0-1.0

//! ListOfferings, end to end over HTTP: `GET /v1/offerings`.
//!
//! The subject is the rule ListOfferings follows: it reports the offering catalogue this
//! service publishes for the Region the request was signed for — every entry exactly once,
//! in one stable order, each field derived from that entry's terms — a page at a time, and
//! reports NONE of them while the account holds a reservation that has started and has not
//! expired. The cases below therefore vary what that rule generalises over: the Region the
//! request is signed for, the page size (across the modelled `MaxResults` range and its
//! default), the form of the pagination token, and the state and the start/end instants of
//! the reservation the account holds.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! door uses, so what these tests assert is what a MediaConnect client sees: routing,
//! restJson1 deserialization, constraint validation, handler, serialization.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::ResourceSpecification;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::reservation_listing::{
    instant_from, CURRENCY_CODE, DURATION_UNITS, ONE_MONTH, PRICE_UNITS, RESOURCE_TYPE,
};

/// `pub(crate)` because the DescribeOffering suite drives the same listing to learn which
/// offerings the service publishes, rather than restating the catalogue.
pub(crate) async fn list_offerings(
    emulator: &Emulator,
    query: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/offerings{query}"), None)
        .await
}

/// The `Offerings` member of a ListOfferings response. See [`list_offerings`] for why this
/// is `pub(crate)`.
pub(crate) fn listed(body: &Value) -> Vec<Value> {
    body["offerings"].as_array().cloned().unwrap_or_default()
}

/// The ARNs of the `Offerings` member, in the order the response carries them. See
/// [`list_offerings`] for why this is `pub(crate)`.
pub(crate) fn listed_arns(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|offering| {
            offering["offeringArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every `OfferingArn` the catalogue carries travels as an `@httpLabel` on DescribeOffering
/// and PurchaseOffering, and is the key ListOfferings pages from, so it must be one this
/// service could have minted in `region`.
///
/// The check runs through the crate's own encoding of that form — the same function
/// ListOfferings validates a pagination token with — and then re-asserts, field by field,
/// the parts the form fixes, so a bug in that function cannot hide here: an offering ARN
/// carries NO account-id field (the catalogue belongs to the service, not to one account)
/// and ends in a numeric offering id.
///
/// `pub(crate)` because the DescribeOffering suite asserts the same form of the ARNs it
/// addresses.
pub(crate) fn assert_minted_offering_arn(case: &str, arn: &str, region: &str) {
    assert!(
        mc::is_mintable_offering_arn(arn),
        "case {case:?}: a listed OfferingArn must be one this service could mint, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: an offering ARN has 6 colon-separated fields, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[4]),
        ("arn", "mediaconnect", region, "offering"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        !segments[5].is_empty() && segments[5].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the last field is a numeric offering id, got {arn:?}"
    );
    assert!(
        !arn.contains(&format!(":{}:", engine::emulator_account_id())),
        "case {case:?}: an offering ARN carries no account-id field, got {arn:?}"
    );
}

/// `Offering$PricePerUnit` as an integer count of thousandths of a dollar, which is what a
/// rate quoted to three decimal places is.
///
/// The parse is also the assertion on the member's FORM: a price is a decimal string, and
/// the catalogue quotes every one of them to the same precision, so a reader can compare
/// two rates without knowing the tier.
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

/// ListOfferings reports the published catalogue: every entry exactly once, with the terms
/// that entry is offered on. Nothing in the response is fixed in the handler — each member
/// below is asserted against the entry's own reserved bitrate, or against the one value the
/// model's enum for it carries.
#[tokio::test]
async fn list_reports_every_published_offering_with_the_terms_of_its_tier() {
    let emulator = Emulator::new();

    let (status, code, body) = list_offerings(&emulator, "?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing expected 200, got {status} {code:?}: {body}"
    );

    let offerings = listed(&body);
    assert_eq!(
        offerings.len(),
        mc::OFFERING_CATALOGUE.len(),
        "every published offering is listed exactly once, got {body}"
    );

    // The amounts of outbound bandwidth the catalogue reserves, as the listing reports
    // them: the set is the published ladder, each entry appearing once.
    let listed_bitrates: Vec<i64> = offerings
        .iter()
        .map(|offering| {
            offering["resourceSpecification"]["reservedBitrate"]
                .as_i64()
                .unwrap_or_default()
        })
        .collect();
    let published_bitrates: Vec<i64> = mc::OFFERING_CATALOGUE
        .iter()
        .map(|entry| i64::from(entry.reserved_bitrate))
        .collect();
    let mut sorted = listed_bitrates.clone();
    sorted.sort_unstable();
    let mut published_sorted = published_bitrates.clone();
    published_sorted.sort_unstable();
    assert_eq!(
        sorted, published_sorted,
        "the listing covers the published bandwidth ladder exactly once each, got {body}"
    );

    // Every entry's price, paired with the bandwidth it reserves, so the catalogue's own
    // invariants can be asserted across the whole ladder below.
    let mut priced: Vec<(i64, i64)> = Vec::new();

    for (offering, bitrate) in offerings.iter().zip(&listed_bitrates) {
        let case = format!("the {bitrate} Mbps offering");
        let case = case.as_str();
        let arn = offering["offeringArn"].as_str().unwrap_or_default();
        assert_minted_offering_arn(case, arn, REGION);
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
             got {offering}"
        );
        let duration = offering["duration"].as_i64().unwrap_or_default();
        assert!(
            duration > 0,
            "case {case:?}: an offering runs for a positive number of months, got {offering}"
        );
        let price_per_unit = offering["pricePerUnit"].as_str().unwrap_or_default();
        let price = price_in_thousandths(case, price_per_unit);
        assert!(
            price > 0 && *bitrate > 0,
            "case {case:?}: an offering reserves bandwidth at a positive price, got {offering}"
        );
        priced.push((*bitrate, price));
        // "A description of the offering": the wording the service publishes a reservation
        // offering under, with the bandwidth and the price of THIS entry in it — both read
        // back out of the entry's own members, so a description quoting another entry's
        // terms, or a wording of its own, fails here.
        assert_eq!(
            offering["offeringDescription"].as_str(),
            Some(
                format!("Reserved outbound bandwidth of {bitrate}Mbps at ${price_per_unit}/hour")
                    .as_str()
            ),
            "case {case:?}: the description states this entry's own terms, got {offering}"
        );
    }

    // What a published reservation price list fixes about its entries: a larger
    // commitment costs more in total, and never more per Mbps than a smaller one — so no
    // entry can be priced off the ladder, in either direction, without a number being
    // written down here.
    priced.sort_unstable();
    for pair in priced.windows(2) {
        let [(smaller, cheaper), (larger, dearer)] = pair else {
            continue;
        };
        let case = format!("the {smaller} Mbps and {larger} Mbps offerings");
        assert!(
            dearer > cheaper,
            "case {case:?}: reserving more bandwidth costs more per hour, got \
             {cheaper} and {dearer} thousandths"
        );
        assert!(
            dearer * smaller <= cheaper * larger,
            "case {case:?}: the rate per Mbps does not rise with the amount reserved, got \
             {cheaper} thousandths for {smaller} Mbps and {dearer} for {larger}"
        );
    }

    // A read does not change what a later caller sees: the catalogue is the same on the
    // next call, in the same order.
    let (_, _, again) = list_offerings(&emulator, "?maxResults=1000").await;
    assert_eq!(
        again, body,
        "listing twice reports the same catalogue, got {again}"
    );
}

/// An offering is "available to this account in the current Amazon Web Services Region",
/// and the Region is a segment of the ARN, so the listing is minted in the Region the
/// request was signed for — the same catalogue entries, addressed in that Region.
#[tokio::test]
async fn the_catalogue_is_listed_in_the_region_the_request_was_signed_for() {
    let emulator = Emulator::new();

    let mut ids_per_region: Vec<Vec<String>> = Vec::new();
    // Rows: the Region every other suite signs for, one in another partition's geography,
    // and one with a digit in its name.
    for region in [REGION, "eu-west-2", "ap-southeast-4"] {
        let (status, code, body) = emulator
            .send_signed_for(region, "GET", "/v1/offerings?maxResults=1000", None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {region:?}: listing expected 200, got {status} {code:?}: {body}"
        );
        let arns = listed_arns(&body);
        assert_eq!(
            arns.len(),
            mc::OFFERING_CATALOGUE.len(),
            "case {region:?}: the whole catalogue is listed, got {body}"
        );
        for arn in &arns {
            assert_minted_offering_arn(region, arn, region);
        }
        ids_per_region.push(
            arns.iter()
                .map(|arn| arn.rsplit(':').next().unwrap_or_default().to_string())
                .collect(),
        );
    }

    // The entries themselves do not depend on the Region: only the ARN's Region segment
    // does, so the offering ids are the same list in the same order.
    for ids in &ids_per_region {
        assert_eq!(
            ids, &ids_per_region[0],
            "the catalogue holds the same offerings in every Region, got {ids:?}"
        );
    }
}

/// One reservation the account holds while ListOfferings is called, and whether it leaves
/// the account eligible for offerings.
struct EligibilityCase {
    case: &'static str,
    name: &'static str,
    /// `Reservation$Start`: `Ok` that long AFTER now (the reservation has not started),
    /// `Err` that long BEFORE it (it has).
    start_from_now: Result<Duration, Duration>,
    /// `Reservation$End`, read the same way: `Ok` is still ahead, `Err` has passed.
    end_from_now: Result<Duration, Duration>,
    stored_state: &'static str,
    /// Whether the catalogue is offered to an account holding it.
    eligible: bool,
}

/// Put one reservation straight into the store, the way a purchase stores one, and answer
/// its ARN.
///
/// PurchaseOffering is still a scaffold
/// (`issues/things-that-do-not-work/20261024T010542Z-mediaconnect-purchaseoffering-stub.md`),
/// so there is no request that mints a reservation yet — the same reason
/// `reservation_listing::seed_reservation` seeds the store directly. This seeding is
/// parameterised on what eligibility turns on instead: the two instants the agreement runs
/// between, and the state the record is in.
fn seed_reservation(
    emulator: &Emulator,
    case: &EligibilityCase,
    now: SystemTime,
    offering: &Value,
) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let suffix = store.next_suffix();
    let reservation_arn = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::reservation_resource_fragment(suffix, case.name),
    );
    // The terms of the agreement are the ones the offering was listed on: a reservation
    // copies them from the offering it was bought from.
    store.reservations.insert(
        reservation_arn.clone(),
        mc::StoredReservation {
            reservation_arn: reservation_arn.clone(),
            reservation_name: case.name.to_string(),
            offering_arn: offering["offeringArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            offering_description: offering["offeringDescription"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            currency_code: CURRENCY_CODE.to_string(),
            price_per_unit: offering["pricePerUnit"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            price_units: PRICE_UNITS.to_string(),
            duration: offering["duration"]
                .as_i64()
                .and_then(|duration| i32::try_from(duration).ok())
                .unwrap_or_default(),
            duration_units: DURATION_UNITS.to_string(),
            resource_specification: ResourceSpecification {
                reserved_bitrate: offering["resourceSpecification"]["reservedBitrate"]
                    .as_i64()
                    .and_then(|bitrate| i32::try_from(bitrate).ok()),
                resource_type: RESOURCE_TYPE.to_string(),
            },
            start_at: instant_from(now, case.start_from_now),
            end_at: instant_from(now, case.end_from_now),
            reservation_state: case.stored_state.to_string(),
            pending: None,
        },
    );
    reservation_arn
}

/// "If you have an active reservation (which means you've purchased an offering that has
/// already started and hasn't expired yet), your account isn't eligible for other
/// offerings": the rows below are the ways a stored reservation can sit against that
/// sentence — inside its window, before it, after it, and ended early — and each is checked
/// against the whole catalogue, not against one entry.
#[tokio::test]
async fn an_account_inside_a_reservations_window_is_offered_nothing() {
    let cases = [
        EligibilityCase {
            case: "an active reservation inside its window",
            name: "res-active",
            start_from_now: Err(ONE_MONTH),
            end_from_now: Ok(ONE_MONTH),
            stored_state: mc::RESERVATION_STATE_ACTIVE,
            eligible: false,
        },
        EligibilityCase {
            case: "a purchase that has started but not settled yet",
            name: "res-processing",
            start_from_now: Err(Duration::from_secs(1)),
            end_from_now: Ok(ONE_MONTH * 12),
            stored_state: mc::RESERVATION_STATE_PROCESSING,
            eligible: false,
        },
        EligibilityCase {
            case: "a reservation whose start is still ahead",
            name: "res-not-started",
            start_from_now: Ok(ONE_MONTH),
            end_from_now: Ok(ONE_MONTH * 13),
            stored_state: mc::RESERVATION_STATE_PROCESSING,
            eligible: true,
        },
        EligibilityCase {
            case: "a reservation whose End has passed",
            name: "res-expired",
            start_from_now: Err(ONE_MONTH * 13),
            end_from_now: Err(ONE_MONTH),
            stored_state: mc::RESERVATION_STATE_EXPIRED,
            eligible: true,
        },
        EligibilityCase {
            case: "an active reservation the clock has run past",
            name: "res-ended",
            start_from_now: Err(ONE_MONTH * 12),
            end_from_now: Err(Duration::from_secs(1)),
            stored_state: mc::RESERVATION_STATE_ACTIVE,
            eligible: true,
        },
        EligibilityCase {
            case: "a canceled reservation inside what was its window",
            name: "res-canceled",
            start_from_now: Err(ONE_MONTH),
            end_from_now: Ok(ONE_MONTH),
            stored_state: mc::RESERVATION_STATE_CANCELED,
            eligible: true,
        },
    ];

    for case in &cases {
        let name_of_case = case.case;
        let emulator = Emulator::new();
        let now = SystemTime::now();

        // The catalogue as it is offered before anything is reserved: the entry the
        // reservation is seeded from, and what the listing must report again when the
        // account stays eligible.
        let (_, _, eligible_body) = list_offerings(&emulator, "?maxResults=1000").await;
        let bought = listed(&eligible_body)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {name_of_case:?}: the catalogue is not empty"));
        seed_reservation(&emulator, case, now, &bought);

        let (status, code, body) = list_offerings(&emulator, "?maxResults=1000").await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {name_of_case:?}: listing expected 200, got {status} {code:?}: {body}"
        );
        if case.eligible {
            assert_eq!(
                body, eligible_body,
                "case {name_of_case:?}: the account is eligible, so the whole catalogue is \
                 offered, got {body}"
            );
        } else {
            assert_eq!(
                listed(&body),
                Vec::<Value>::new(),
                "case {name_of_case:?}: an account with an active reservation is offered no \
                 offering, got {body}"
            );
            assert_eq!(
                body["nextToken"].as_str(),
                None,
                "case {name_of_case:?}: an empty listing carries no token, got {body}"
            );
        }
    }
}

/// Eligibility is owed by the CLOCK, not by a call: while a reservation is running the
/// account is offered nothing, and from its `End` on every read offers the catalogue again
/// — without any call in between moving the reservation.
#[tokio::test]
async fn the_clock_makes_the_account_eligible_again_at_the_reservations_end() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    // Short enough that the test does not wait long, and long enough that the first read
    // lands before it.
    let runs_for = Duration::from_millis(400);

    let (_, _, catalogue) = list_offerings(&emulator, "?maxResults=1000").await;
    let bought = listed(&catalogue)
        .into_iter()
        .next()
        .expect("the catalogue is not empty");
    let case = EligibilityCase {
        case: "a reservation ending during the test",
        name: "res-expiring",
        start_from_now: Err(Duration::from_secs(60)),
        end_from_now: Ok(runs_for),
        stored_state: mc::RESERVATION_STATE_ACTIVE,
        eligible: false,
    };
    seed_reservation(&emulator, &case, now, &bought);

    let (_, _, during) = list_offerings(&emulator, "?maxResults=1000").await;
    assert_eq!(
        listed(&during),
        Vec::<Value>::new(),
        "while the reservation runs the account is offered nothing, got {during}"
    );

    tokio::time::sleep(runs_for + Duration::from_millis(100)).await;
    let (status, code, after) = list_offerings(&emulator, "?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing after the End expected 200, got {status} {code:?}: {after}"
    );
    assert_eq!(
        after, catalogue,
        "past its End the reservation holds nothing, so the catalogue is offered again, got \
         {after}"
    );
}

/// ListOfferings pages the catalogue: whatever `MaxResults` is, the pages partition every
/// published offering exactly once, in one stable order, and only the last page omits
/// `NextToken`. With no `MaxResults` the page holds the documented default of ten.
#[tokio::test]
async fn list_pages_every_published_offering_exactly_once() {
    let emulator = Emulator::new();

    let (_, _, all) = list_offerings(&emulator, "?maxResults=1000").await;
    let expected = listed_arns(&all);
    assert!(
        expected.len() > 10,
        "the catalogue is larger than the default page, so the default is observable, got \
         {expected:?}"
    );

    // "If MaxResults is not included in the request, the service defaults to pagination
    // with a maximum of 10 results per page"
    // (`smithy_query(query="doc",shape="ListOfferingsRequest",member="MaxResults")`).
    let (status, _, default_page) = list_offerings(&emulator, "").await;
    assert_eq!(status, 200, "the default page expected 200, got {status}");
    assert_eq!(
        listed_arns(&default_page),
        expected[..10].to_vec(),
        "the default page holds the first ten published offerings, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(expected[10].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled `MaxResults` range (1..1000): a page smaller than
    // the catalogue, ones that divide it, one that equals it, and the maximum.
    for page_size in [1_usize, 2, 4, expected.len(), 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, code, body) = list_offerings(&emulator, &query).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "page size {page_size}: expected 200, got {status} {code:?}: {body}"
            );
            let page = listed_arns(&body);
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    page_size,
                    "page size {page_size}: a page that carries a token is full, got {body}"
                );
            }
            seen.extend(page);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: the pages must partition every published offering once, \
             in order"
        );
    }

    // The token is the ARN the listing RESUMES AT, so a page resumed from it starts there.
    let (_, _, first_page) = list_offerings(&emulator, "?maxResults=3").await;
    let token = first_page["nextToken"]
        .as_str()
        .expect("a page smaller than the catalogue carries a token")
        .to_string();
    let (status, code, resumed) =
        list_offerings(&emulator, &format!("?maxResults=1000&nextToken={token}")).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "resuming at an offering's ARN expected 200, got {status} {code:?}: {resumed}"
    );
    assert_eq!(
        listed_arns(&resumed),
        expected[3..].to_vec(),
        "the listing resumes at the key the token names, got {resumed}"
    );
}

/// One refused ListOfferings request.
struct RefusalCase {
    case: &'static str,
    query: String,
    status: u16,
    code: &'static str,
}

/// ListOfferings refuses a page size outside the modelled range and a pagination token it
/// could not have issued — with the modelled `BadRequestException` (400), never with a 501
/// and never with a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    let cases = vec![
        // The modelled `MaxResults` range is 1..1000, so each bound is usable (the paging
        // suite above lists with 1 and with 1000) and each value past it is refused.
        RefusalCase {
            case: "MaxResults below the modelled range",
            query: "?maxResults=0".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "MaxResults above the modelled range",
            query: "?maxResults=1001".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        // A `NextToken` is the offering ARN the listing resumes at, so a token the service
        // could not have minted is refused. The rows are the ways a token can fail that
        // form: not an ARN at all, another service's ARN, an account-id field the published
        // form does not carry, the wrong resource type, an empty offering id, an offering
        // id that is not numeric, and an id carrying a further ARN segment.
        RefusalCase {
            case: "a nextToken that is not an ARN",
            query: "?nextToken=not-a-token".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken from another service",
            query: "?nextToken=arn:aws:medialive:us-east-1:offering:12000100".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken carrying an account-id field",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:offering:12000100"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken naming another resource type",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:flow:12000100".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken with an empty offering id",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:offering:".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken whose offering id is not numeric",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:offering:1-deadbeef".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken carrying a further ARN segment",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:offering:12000100:more".to_string(),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        query,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, body) = list_offerings(&emulator, &query).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body}"
        );
    }

    // A token the service COULD have minted is accepted even when the catalogue holds no
    // such entry: paging is a walk from a key, not a lookup. This one sorts past every
    // published ARN, so nothing is left to report.
    let unknown = "arn:aws:mediaconnect:us-east-1:offering:99999999";
    let (status, code, body) = list_offerings(&emulator, &format!("?nextToken={unknown}")).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a well-formed token for an unknown offering pages from it, got {status} {code:?}: {body}"
    );
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "nothing is published past that key, got {body}"
    );
}
