// SPDX-License-Identifier: CC0-1.0

//! `ListReservations`: which of the account's purchased reservations a request
//! reports, what each reported reservation carries, and how a page is cut — end
//! to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.
//!
//! The rules asserted here, each over a TABLE spanning its class rather than one
//! request — an unfiltered `ListReservations` call, the case the stub issue
//! reproduces, is ONE row of them:
//!
//! 1. A listing reports every reservation the account has purchased, once each,
//!    in the order their ids were minted, and reports each one exactly as the
//!    keyed reader (`DescribeReservation`) and the purchase itself report it; an
//!    account that has bought nothing reports nothing.
//! 2. Each filter admits exactly the reservations whose reserved configuration
//!    carries that value, filters combine by intersection, and a value its member
//!    does not declare is refused rather than silently admitting nothing.
//! 3. A page carries as many reservations as `MaxResults` asks for while
//!    reservations are left to report, bounded by the service's published page
//!    ceiling, and `NextToken` resumes the walk at the reservation left over.
//! 4. A request this operation cannot serve as asked is its modelled 400 and
//!    reports no reservations.
//!
//! The expected sets are RE-DERIVED here from the model's own value sets (the
//! `*_VALUES` constants generated for each `ReservationResourceSpecification`
//! member) and from what the purchases answered with, so this suite agrees with
//! the model rather than with the handler.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::ops::types::{
    CHANNEL_CLASS_VALUES, RESERVATION_CODEC_VALUES, RESERVATION_MAXIMUM_BITRATE_VALUES,
    RESERVATION_MAXIMUM_FRAMERATE_VALUES, RESERVATION_RESOLUTION_VALUES,
    RESERVATION_RESOURCE_TYPE_VALUES, RESERVATION_SPECIAL_FEATURE_VALUES,
    RESERVATION_VIDEO_QUALITY_VALUES,
};
use service_com_amazonaws_medialive_medialive::state::{MAX_LIST_PAGE_SIZE, MAX_RESERVATIONS};
use smithy_server_runtime::MultiServiceRouter;

use crate::reservation_purchase::{
    describe, first_offering, purchase, reservation, RESERVATION_MEMBERS,
};
use crate::wire::{call, error_code, medialive, DEFAULT_REGION};

/// The collection this listing is bound to (`smithy_query(query="operation",
/// operation="ListReservations")`: GET /prod/reservations).
const RESERVATIONS: &str = "/prod/reservations";

/// The member a page of reservations is reported in (the `jsonName` of
/// `ListReservationsResponse$Reservations`).
const ITEMS: &str = "reservations";

/// The eight filters this request carries, each named after the
/// `ReservationResourceSpecification` member it filters on, beside the value set
/// that member's own enum declares (`smithy_query(query="shape",
/// shape="ReservationResourceSpecification")`, whose members target
/// `ChannelClass`, `ReservationCodec`, `ReservationMaximumBitrate`,
/// `ReservationMaximumFramerate`, `ReservationResolution`,
/// `ReservationResourceType`, `ReservationSpecialFeature` and
/// `ReservationVideoQuality`; the `*_VALUES` constants are the ones the model
/// generates for them).
const FILTERS: [(&str, &[&str]); 8] = [
    ("channelClass", CHANNEL_CLASS_VALUES),
    ("codec", RESERVATION_CODEC_VALUES),
    ("maximumBitrate", RESERVATION_MAXIMUM_BITRATE_VALUES),
    ("maximumFramerate", RESERVATION_MAXIMUM_FRAMERATE_VALUES),
    ("resolution", RESERVATION_RESOLUTION_VALUES),
    ("resourceType", RESERVATION_RESOURCE_TYPE_VALUES),
    ("specialFeature", RESERVATION_SPECIAL_FEATURE_VALUES),
    ("videoQuality", RESERVATION_VIDEO_QUALITY_VALUES),
];

/// One reservation this suite's account holds: the case naming it, the id the
/// store minted for it and the reservation the purchase answered with.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    id: String,
    purchased: Value,
}

/// The `ReservationId` a reported reservation carries, as a client reads it.
fn id_of(case: &str, reported: &Value) -> String {
    reported["reservationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted reservationId, got {reported}"))
        .to_string()
}

/// One member of a reported reservation's `ResourceSpecification`, as a client
/// reads it — `None` when the reserved configuration does not carry that member
/// at all (the model scopes `MaximumFramerate` and `VideoQuality` to outputs and
/// `SpecialFeature` to channels).
fn specification_member(reported: &Value, member: &str) -> Option<String> {
    reported
        .get("resourceSpecification")?
        .get(member)?
        .as_str()
        .map(ToString::to_string)
}

/// The query string a client sends: the filters, plus the paging members (all of
/// them `@httpQuery`).
fn query(filters: &str, max_results: Option<usize>, next_token: Option<&str>) -> String {
    let mut query = String::new();
    for parameter in [
        (!filters.is_empty()).then(|| filters.to_string()),
        max_results.map(|size| format!("maxResults={size}")),
        next_token.map(|token| format!("nextToken={token}")),
    ]
    .into_iter()
    .flatten()
    {
        query.push(if query.is_empty() { '?' } else { '&' });
        query.push_str(&parameter);
    }
    query
}

/// One `ListReservations` call.
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{RESERVATIONS}{query}"), None).await
}

/// The reported page as a list, whatever its length. An empty list and an absent
/// member both mean "no entries" to a client, and which of the two the restJson1
/// codec writes is the serializer's choice, so both read here as the empty list.
fn items(case: &str, body: &Value) -> Vec<Value> {
    match body.get(ITEMS) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a {ITEMS} list, got {other}"),
    }
}

/// Every reservation a filtered walk reports, following `NextToken` to the end.
///
/// The token discipline is asserted on the way: a page never carries more than
/// the request asked for, no token comes back beside an empty page, and the walk
/// terminates within the store's own ceiling.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    filters: &str,
    max_results: Option<usize>,
) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, body) = list(router, &query(filters, max_results, token.as_deref())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListReservations to answer 200, got {status}: {body}"
        );
        let page = items(case, &body);
        let next = body["nextToken"].as_str().map(ToString::to_string);
        if let Some(size) = max_results {
            assert!(
                page.len() <= size,
                "case {case:?}: expected at most {size} reservations in a page, got {}",
                page.len()
            );
        }
        if next.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?}: expected no nextToken beside an empty page, got {body}"
            );
        }
        reported.extend(page);
        assert!(
            reported.len() <= MAX_RESERVATIONS,
            "case {case:?}: expected the walk to terminate within the store's {MAX_RESERVATIONS} \
             reservations, got {} reported",
            reported.len()
        );
        match next {
            Some(next) => token = Some(next),
            None => return reported,
        }
    }
}

/// What this suite's account holds: one reservation bought against the first
/// offering carrying EVERY value EVERY specification member declares, so each
/// filter below has reservations to admit and reservations to leave out, and the
/// rows differ along the configuration reserved and the `Count` reserved.
///
/// Well within the store's own ceiling ([`MAX_RESERVATIONS`]), which the walk
/// above relies on.
async fn purchased_rows(router: &MultiServiceRouter) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (member, values) in FILTERS {
        for value in values {
            let case = format!("the first offering with {member}={value}");
            let offering_id = first_offering(router, &case, &format!("{member}={value}")).await;
            let body = json!({
                "count": 1 + (rows.len() % 5),
                "name": format!("{member}-{value}"),
            });
            let (status, answered) = purchase(router, DEFAULT_REGION, &offering_id, body).await;
            assert_eq!(
                status, 201,
                "setup {case:?}: expected PurchaseOffering to answer 201, got {status}: {answered}"
            );
            let purchased = reservation(&case, &answered);
            let id = id_of(&case, &purchased);
            rows.push(Row {
                case,
                id,
                purchased,
            });
        }
    }
    assert!(
        rows.len() <= MAX_RESERVATIONS,
        "setup: expected the table to fit the store's {MAX_RESERVATIONS} reservations, got {}",
        rows.len()
    );
    rows
}

/// Rule 1: a listing reports every reservation the account has purchased, once
/// each, in the order their ids were minted, and reports each one exactly as the
/// purchase answered and as `DescribeReservation` reports it; an account that has
/// bought nothing reports nothing, and a listing is a report, so making it again
/// answers the same reservations.
#[tokio::test]
async fn a_listing_reports_every_purchased_reservation_as_the_keyed_reader_does() {
    let router = medialive();

    let empty = every_page(&router, "an account holding no reservations", "", None).await;
    assert!(
        empty.is_empty(),
        "case \"an account holding no reservations\": expected nothing to report, got {} \
         ({:?})",
        empty.len(),
        empty.first()
    );

    let rows = purchased_rows(&router).await;
    let listed = every_page(&router, "every reservation held", "", None).await;
    let listed_ids: Vec<String> = listed
        .iter()
        .map(|reported| id_of("every reservation held", reported))
        .collect();
    let minted: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    assert_eq!(
        listed_ids, minted,
        "case \"every reservation held\": expected the listing to report every purchased \
         reservation once, in mint order"
    );

    for (row, reported) in rows.iter().zip(listed.iter()) {
        let (status, described) = describe(&router, DEFAULT_REGION, &row.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeReservation to answer 200, got {status}: {described}",
            row.case
        );
        for member in RESERVATION_MEMBERS {
            assert_eq!(
                reported.get(member),
                row.purchased.get(member),
                "case {:?}: expected the listed {member} to be the purchased one, got {:?} \
                 against {:?}",
                row.case,
                reported.get(member),
                row.purchased.get(member)
            );
            assert_eq!(
                reported.get(member),
                described.get(member),
                "case {:?}: expected the listed {member} to be what the keyed reader reports, \
                 got {:?} against {:?}",
                row.case,
                reported.get(member),
                described.get(member)
            );
        }
    }

    // A report changes nothing a later caller sees: the same walk, cut into
    // pages of a different size, answers the same reservations.
    let again = every_page(&router, "the listing repeated", "", Some(3)).await;
    assert_eq!(
        again, listed,
        "case \"the listing repeated\": expected a repeated listing to report the same \
         reservations"
    );
}

/// Rule 2: each filter admits exactly the reservations whose reserved
/// configuration carries that value, and filters combine by intersection — so a
/// request naming every member of one reservation's configuration admits the
/// reservations of that configuration, and one naming a member another resource
/// type does not carry admits none.
///
/// The table spans EVERY filter and EVERY value its own member declares, each
/// checked against the same predicate applied to what the purchases answered
/// with, so no value is special-cased.
#[tokio::test]
async fn each_filter_admits_exactly_the_reservations_reserving_its_value() {
    let router = medialive();
    let rows = purchased_rows(&router).await;

    for (member, values) in FILTERS {
        for value in values.iter().copied() {
            let case = format!("{member}={value}");
            let expected: BTreeSet<String> = rows
                .iter()
                .filter(|row| {
                    specification_member(&row.purchased, member).as_deref() == Some(value)
                })
                .map(|row| row.id.clone())
                .collect();
            assert!(
                !expected.is_empty(),
                "case {case:?}: expected the account to hold a reservation reserving every \
                 modelled value"
            );

            let admitted: BTreeSet<String> = every_page(&router, &case, &case, None)
                .await
                .iter()
                .map(|reported| id_of(&case, reported))
                .collect();
            assert_eq!(
                admitted, expected,
                "case {case:?}: expected the filter to admit exactly the {} reservations \
                 reserving that value, got {}",
                expected.len(),
                admitted.len()
            );
        }
    }

    // Filters COMBINE: a request naming every member one reservation's
    // configuration carries admits the reservations of that configuration and no
    // others.
    let row = rows
        .last()
        .expect("the table holds at least one reservation");
    let combined: String = FILTERS
        .iter()
        .filter_map(|(member, _)| {
            specification_member(&row.purchased, member).map(|value| format!("{member}={value}"))
        })
        .collect::<Vec<String>>()
        .join("&");
    let expected: BTreeSet<String> = rows
        .iter()
        .filter(|other| {
            other.purchased.get("resourceSpecification")
                == row.purchased.get("resourceSpecification")
        })
        .map(|other| other.id.clone())
        .collect();
    let admitted: BTreeSet<String> = every_page(&router, &combined, &combined, None)
        .await
        .iter()
        .map(|reported| id_of(&combined, reported))
        .collect();
    assert_eq!(
        admitted, expected,
        "case {combined:?}: expected the combined filters to admit exactly the {} reservations \
         reserving that configuration, got {}",
        expected.len(),
        admitted.len()
    );

    // A member the model scopes to ONE resource type is carried by no
    // reservation of another (`MaximumFramerate` and `VideoQuality` are
    // "(Outputs only)", `SpecialFeature` is "(Channels only)"), so naming both
    // admits nothing.
    let framerate = RESERVATION_MAXIMUM_FRAMERATE_VALUES
        .first()
        .expect("the model declares a MaximumFramerate");
    let video_quality = RESERVATION_VIDEO_QUALITY_VALUES
        .first()
        .expect("the model declares a VideoQuality");
    let special_feature = RESERVATION_SPECIAL_FEATURE_VALUES
        .first()
        .expect("the model declares a SpecialFeature");
    for case in [
        format!("resourceType=INPUT&maximumFramerate={framerate}"),
        format!("resourceType=INPUT&videoQuality={video_quality}"),
        format!("resourceType=MULTIPLEX&specialFeature={special_feature}"),
    ] {
        let admitted = every_page(&router, &case, &case, None).await;
        assert!(
            admitted.is_empty(),
            "case {case:?}: expected a member scoped to another resource type to admit no \
             reservation, got {} ({:?})",
            admitted.len(),
            admitted.first()
        );
    }
}

/// Rule 3: a page carries as many reservations as `MaxResults` asks for while
/// reservations are left to report — up to the service's published page ceiling,
/// which is also the size a request naming no `MaxResults` gets — a `NextToken`
/// comes back only when a reservation is left over, and however the pages were
/// cut the walk reports every held reservation exactly once in mint order.
///
/// The rows differ along the asked-for page size relative to both what the
/// account holds and that ceiling, because that is what the rule generalises
/// over, and the last row cuts a FILTERED walk, where the page is cut over the
/// admitted reservations rather than over the store.
#[tokio::test]
async fn a_page_carries_as_many_reservations_as_the_request_asks_for() {
    let router = medialive();
    let rows = purchased_rows(&router).await;
    let held = rows.len();
    let minted: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();

    for max_results in [
        None,
        Some(1),
        Some(2),
        Some(5),
        Some(held - 1),
        Some(held),
        Some(held + 1),
        Some(MAX_LIST_PAGE_SIZE),
        Some(MAX_LIST_PAGE_SIZE + 500),
    ] {
        let case = format!("maxResults={max_results:?}");
        let cut_at = max_results
            .unwrap_or(MAX_LIST_PAGE_SIZE)
            .min(MAX_LIST_PAGE_SIZE);
        let expected_first = cut_at.min(held);

        let (status, first) = list(&router, &query("", max_results, None)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListReservations to answer 200, got {status}: {first}"
        );
        let page = items(&case, &first);
        assert_eq!(
            page.len(),
            expected_first,
            "case {case:?}: expected a page of {expected_first} reservations out of {held} held, \
             got {}",
            page.len()
        );
        assert_eq!(
            first["nextToken"].as_str().is_some(),
            expected_first < held,
            "case {case:?}: expected a nextToken only with reservations left over, got {first}"
        );

        let walked: Vec<String> = every_page(&router, &case, "", max_results)
            .await
            .iter()
            .map(|reported| id_of(&case, reported))
            .collect();
        assert_eq!(
            walked, minted,
            "case {case:?}: expected the walk to report every held reservation once, in mint order"
        );
    }

    // The cut is made over the ADMITTED reservations: a filtered walk in pages
    // of one reports exactly the reservations that filter admits, in the same
    // order.
    let resource_type = RESERVATION_RESOURCE_TYPE_VALUES
        .first()
        .expect("the model declares a ReservationResourceType");
    let case = format!("resourceType={resource_type}");
    let expected: Vec<String> = rows
        .iter()
        .filter(|row| {
            specification_member(&row.purchased, "resourceType").as_deref() == Some(resource_type)
        })
        .map(|row| row.id.clone())
        .collect();
    let paged: Vec<String> = every_page(&router, &case, &case, Some(1))
        .await
        .iter()
        .map(|reported| id_of(&case, reported))
        .collect();
    assert_eq!(
        paged, expected,
        "case {case:?}: expected a filtered walk in pages of one to report the {} admitted \
         reservations in mint order, got {}",
        expected.len(),
        paged.len()
    );
}

/// Rule 4: a request this operation cannot serve as asked is refused with its
/// modelled `BadRequestException` (400) and reports no reservations — a page size
/// that asks for nothing, and a filter value outside the set its member declares.
///
/// The rows differ along WHICH member is unserveable and how, because that is
/// what the rule generalises over; the admissible bound beside them (the smallest
/// page size, and every modelled value of every filter, asserted by the suite
/// above) is what keeps the refusal from being a blanket rejection.
#[tokio::test]
async fn a_request_this_operation_cannot_serve_is_refused() {
    let router = medialive();
    // The account HOLDS reservations, so a refusal is not just "nothing to
    // report".
    let rows = purchased_rows(&router).await;

    let mut cases: Vec<String> = vec![
        "maxResults=0".to_string(),
        "maxResults=-1".to_string(),
        "maxResults=-50".to_string(),
    ];
    for (member, values) in FILTERS {
        let declared = values
            .first()
            .expect("every filtered member declares a value");
        // A value outside the member's modelled set describes no reservable
        // configuration: the model's values are upper case, so the same value in
        // lower case is one of them spelled as the member does not declare it.
        cases.push(format!("{member}={}", declared.to_lowercase()));
        cases.push(format!("{member}={member}-is-not-this"));
    }

    for case in cases {
        let (status, body) = list(&router, &format!("?{case}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert!(
            body.get(ITEMS).is_none(),
            "case {case:?}: expected a refusal to report no reservations, got {body}"
        );
    }

    // The bound itself: the smallest page size this operation admits reports one
    // reservation and a token for the rest, and the refusals above stored
    // nothing.
    let (status, body) = list(&router, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case \"maxResults=1\": expected the smallest page size to succeed, got {status}: {body}"
    );
    assert_eq!(
        items("maxResults=1", &body).len(),
        1,
        "case \"maxResults=1\": expected a page of one, got {body}"
    );
    let walked = every_page(&router, "after the refusals", "", None).await;
    assert_eq!(
        walked.len(),
        rows.len(),
        "case \"after the refusals\": expected the account to still hold its {} reservations, \
         got {}",
        rows.len(),
        walked.len()
    );
}
