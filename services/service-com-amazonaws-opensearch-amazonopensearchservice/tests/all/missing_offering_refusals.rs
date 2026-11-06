// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service refuses an offering id the published
//! Reserved Instance catalog holds no offering under.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness and the per-operation request
//! helpers come from the sibling suites; this one adds no second copy of either.
//!
//! The rule under test, asserted over a SET of inputs rather than over one call:
//! every operation that RESOLVES a `ReservedInstanceOfferingId` against the published
//! catalog answers an id the catalog holds no offering under with the modelled
//! `ResourceNotFoundException`, on the status a missing resource is reported under
//! ([`RECORDED_NOT_FOUND_STATUS`], which is not the status the shape's own
//! `@httpError` declares), stating the KIND of identifier that resolved to nothing
//! and no value — so the catalog listing and a purchase cannot answer one miss two
//! ways. Around it:
//!
//! * the same operations resolve an id the catalog DOES publish, so the refusal is
//!   about resolution and not a blanket answer to the member;
//! * a value that is not a `GUID` at all stays the malformed request it is, which is
//!   a different class with a different status;
//! * the refusals write nothing: no reservation exists after them, and the catalog
//!   answers the next caller exactly as before.

use std::collections::BTreeSet;

use serde_json::json;

use super::control_plane_refusal_wording::message;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, RECORDED_NOT_FOUND_STATUS,
};
use super::reserved_instance_offerings::{describe, offerings, whole_catalog};
use super::reserved_instances::{purchase, reservations};

/// The sentence this service states about an offering id that resolves to nothing.
/// It names the kind of identifier and no value: the id came from the request, so
/// echoing it back tells a caller nothing it does not already hold.
const OFFERING_ID_NOT_FOUND: &str = "Offering id not found";

/// Candidate ids the catalog publishes no offering under, each inside `GUID`'s own
/// constraint (`@length(36, 36)` and the 8-4-4-4-12 `\p{XDigit}` expression, from
/// `smithy_query(query="shape",shape="GUID")`), differing in the FORM of the id and
/// not only in its value: the lowest value the expression admits, the highest, a
/// mixed-case one (hexadecimal digits of either case satisfy it), one whose groups
/// are all decimal digits, and one of ordinary mixed hexadecimal.
fn absent_offering_ids() -> [(&'static str, &'static str); 5] {
    [
        ("every group at its lowest", "00000000-0000-0000-0000-000000000000"),
        ("every group at its highest", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ("upper-case hexadecimal", "ABCDEF01-2345-6789-ABCD-EF0123456789"),
        ("decimal digits only", "12345678-1234-1234-1234-123456789012"),
        ("mixed hexadecimal", "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"),
    ]
}

/// The purchase body an offering id is bought with: a count and a name the modelled
/// constraints admit, so what decides the answer is the offering id alone.
fn purchase_body(offering_id: &str, reservation_name: &str) -> serde_json::Value {
    json!({
        "ReservedInstanceOfferingId": offering_id,
        "ReservationName": reservation_name,
        "InstanceCount": 1,
    })
}

/// Assert `answer` is the missing-offering refusal: the modelled error code, on the
/// status a missing resource is reported under, stating the one sentence and naming
/// no id.
fn assert_offering_not_found(answer: &Answer, case: &str, requested_id: &str) {
    assert_eq!(
        (answer.status, error_code(answer).as_str()),
        (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case {case}: expected {RECORDED_NOT_FOUND_STATUS} ResourceNotFoundException, got {} \
         with {:?}",
        answer.status,
        answer.body
    );
    let said = message(answer);
    assert_eq!(
        said, OFFERING_ID_NOT_FOUND,
        "case {case}: expected {OFFERING_ID_NOT_FOUND:?}, got {said:?}"
    );
    assert!(
        !said.contains(requested_id),
        "case {case}: the sentence names no id, got {said:?}"
    );
}

/// Every id of the published catalog, walked through the continuation tokens the
/// listing hands out.
async fn published_ids(emulator: &Emulator, case: &str) -> BTreeSet<String> {
    whole_catalog(emulator, case)
        .await
        .iter()
        .map(|offering| string_member(offering, "ReservedInstanceOfferingId", case))
        .collect()
}

#[tokio::test]
async fn every_offering_reader_refuses_an_id_the_catalog_does_not_publish() {
    let emulator = Emulator::new();
    let published = published_ids(&emulator, "the published catalog").await;

    for (form, absent) in absent_offering_ids() {
        // The row has to name an offering the catalog does NOT publish, or it would
        // be asserting the resolved case instead.
        assert!(
            !published.contains(absent),
            "case {form}: the row must name no published offering, and the catalog publishes \
             {absent}"
        );

        let filtered = describe(&emulator, &format!("offeringId={absent}")).await;
        assert_offering_not_found(
            &filtered,
            &format!("DescribeReservedInstanceOfferings({form}: {absent})"),
            absent,
        );

        let bought = purchase(&emulator, purchase_body(absent, "no-such-offering")).await;
        assert_offering_not_found(
            &bought,
            &format!("PurchaseReservedInstanceOffering({form}: {absent})"),
            absent,
        );
    }

    // Nothing above was served, so nothing above wrote: the refusals leave this
    // account holding no reservation at all.
    let listing = reservations(&emulator, "").await;
    let held = listing.body["ReservedInstances"].as_array().cloned();
    assert_eq!(
        (listing.status, held.as_ref().map(Vec::len)),
        (200, Some(0)),
        "case after the refused reads: expected an empty reservation listing, got {} with {:?}",
        listing.status,
        listing.body
    );

    // And the catalog answers the next caller exactly as it did the first.
    assert_eq!(
        published_ids(&emulator, "the catalog after the refusals").await,
        published,
        "case after the refused reads: expected the catalog to publish the same offerings"
    );
}

#[tokio::test]
async fn an_offering_the_catalog_publishes_resolves_through_the_same_operations() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "the published catalog").await;

    // The catalog this Region publishes is what the rows below are drawn from, so a
    // listing that reports nothing is a regression in the catalog rather than a row to
    // skip: it is named here instead of underflowing the index arithmetic.
    assert!(
        !catalog.is_empty(),
        "case the published catalog: expected the Region's offerings, got an empty listing"
    );

    // Several offerings of the catalog, drawn from its ends and its middle, so the
    // resolved case is asserted over more than one row of it. A catalog too short to
    // have three distinct rows contributes each of them once, in order.
    let indices: BTreeSet<usize> = [0, catalog.len() / 2, catalog.len() - 1].into();
    for (nth, index) in indices.into_iter().enumerate() {
        let case = format!("offering {index} of {}", catalog.len());
        let id = string_member(&catalog[index], "ReservedInstanceOfferingId", &case);

        let answer = describe(&emulator, &format!("offeringId={id}")).await;
        let reported = offerings(&answer, &case);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: the filter names one offering, got {reported:?}"
        );
        assert_eq!(
            reported[0], catalog[index],
            "case {case}: expected the offering the catalog publishes, got {:?}",
            reported[0]
        );

        // The SAME id a purchase resolves: a reservation is minted and read back
        // through `DescribeReservedInstances`, so the id names something both
        // operations can resolve.
        let name = format!("held-offering-{nth}");
        let bought = purchase(&emulator, purchase_body(&id, &name)).await;
        assert_eq!(
            bought.status, 200,
            "case {case}: expected the purchase of a published offering to be served, got {} \
             with {:?}",
            bought.status, bought.body
        );
        let reserved_id = string_member(&bought.body, "ReservedInstanceId", &case);
        let listing = reservations(&emulator, &format!("?reservationId={reserved_id}")).await;
        let held = listing.body["ReservedInstances"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            (listing.status, held.len()),
            (200, 1),
            "case {case}: expected the reservation to be readable back, got {} with {:?}",
            listing.status,
            listing.body
        );
        assert_eq!(
            string_member(&held[0], "ReservedInstanceOfferingId", &case),
            id,
            "case {case}: expected the reservation to name the offering it was bought from, got \
             {:?}",
            held[0]
        );
    }
}

#[tokio::test]
async fn a_value_that_is_no_guid_stays_the_malformed_request_it_is() {
    let emulator = Emulator::new();

    // Each value breaks `GUID`'s own expression in a different way: a group that is
    // not hexadecimal, groups of the wrong lengths at the modelled length, and a
    // value shorter than the modelled length.
    for (form, malformed) in [
        ("a non-hexadecimal group", "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz"),
        ("groups of the wrong lengths", "123456781-234-1234-1234-123456789abc"),
        ("shorter than the modelled length", "not-a-guid"),
    ] {
        let filtered = describe(&emulator, &format!("offeringId={malformed}")).await;
        let case = format!("DescribeReservedInstanceOfferings({form}: {malformed})");
        assert_eq!(
            (filtered.status, error_code(&filtered).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {} with {:?}",
            filtered.status,
            filtered.body
        );

        let bought = purchase(&emulator, purchase_body(malformed, "malformed-offering")).await;
        let case = format!("PurchaseReservedInstanceOffering({form}: {malformed})");
        assert_eq!(
            (bought.status, error_code(&bought).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {} with {:?}",
            bought.status,
            bought.body
        );
    }
}
