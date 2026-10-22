// SPDX-License-Identifier: CC0-1.0

//! `CreateAddress` validation and the address type it records, driven end to end over
//! HTTP.
//!
//! Three rules, each asserted over a TABLE rather than over one address:
//!
//! 1. An address created for a device to be shipped to is a delivery address unless the
//!    create typed it otherwise, so every stored address carries an `AddressType` and
//!    `DescribeAddress` reports it.
//! 2. An address is validated when it is created: one missing a member the carrier needs
//!    to hand over the device is refused with `InvalidAddressException` (HTTP 400) naming
//!    that member.
//! 3. A destination the Region that took the order does not ship to is refused with HTTP
//!    400 `ValidationException`.
//!
//! The harness (`router`, `call_in_region`, `create_address`, `assert_address_matches`)
//! is the one `job_lifecycle` already drives the service with, so both suites talk to
//! the emulator through the same front door.

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_address::{
    DEFAULT_ADDRESS_TYPE, REQUIRED_DELIVERY_MEMBERS,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::ADDRESS_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    assert_address_matches, assert_prefixed_uuid, call_in_region, create_address,
    deliverable_address, ok, router, string,
};

/// The Region every call that is not about Regions signs for; its shipping area is the
/// United States.
const HOME_REGION: &str = "us-west-2";

/// One `CreateAddress`, signed for `region`.
async fn create_in_region(
    router: &MultiServiceRouter,
    region: &str,
    address: &Value,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "CreateAddress",
        json!({ "Address": address }),
    )
    .await
}

/// Assert a refusal: HTTP 400, the wire error code `error_code`, and an error message in
/// `member` that contains `names`.
fn assert_refused(
    status: u16,
    body: &Value,
    error_code: &str,
    member: &str,
    names: &str,
    case: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 {error_code}, got {status} with {body}"
    );
    let wire_type = body["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(error_code),
        "case {case:?}: expected __type {error_code}, got {body}"
    );
    let message = body[member].as_str().unwrap_or_default();
    assert!(
        message.contains(names),
        "case {case:?}: expected the {member} to name {names:?}, got {body}"
    );
    assert!(
        body.get("AddressId").is_none(),
        "case {case:?}: a refused CreateAddress must mint no AddressId, got {body}"
    );
}

#[tokio::test]
async fn an_address_carries_the_type_it_was_created_with_or_a_delivery_type() {
    let router = router();

    // Each row is one way the create can speak about the address type: silence, and each
    // value the modelled `AddressType` enum admits.
    let mut rows: Vec<(String, Value)> = vec![(
        "no Type at all".to_string(),
        deliverable_address("Untyped Receiver", "1 Default Way", json!({})),
    )];
    for address_type in ADDRESS_TYPE_VALUES {
        rows.push((
            format!("Type {address_type}"),
            deliverable_address(
                "Typed Receiver",
                "2 Typed Way",
                json!({ "Type": address_type }),
            ),
        ));
    }

    for (case, submitted) in rows {
        let address_id = create_address(&router, submitted.clone(), &case).await;
        let described = ok(
            &router,
            "DescribeAddress",
            json!({ "AddressId": address_id }),
            &case,
        )
        .await;
        let reported = &described["Address"];

        // The rule itself: the type is the one the create named, and a create that named
        // none leaves a delivery address.
        let expected = submitted
            .get("Type")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_ADDRESS_TYPE);
        assert_eq!(
            reported.get("Type").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected Address.Type {expected}, got {described}"
        );
        assert!(
            ADDRESS_TYPE_VALUES.contains(&expected),
            "case {case:?}: {expected} is not a value the modelled AddressType admits"
        );
        // And nothing else about the address moved while the type was being recorded.
        assert_address_matches(reported, &submitted, &address_id, &case);
    }
}

#[tokio::test]
async fn create_address_refuses_a_type_outside_the_modelled_catalog() {
    let router = router();

    // Each row is a value `Address$Type` does not admit: a near-miss on a catalog value,
    // the wrong case, the value with its underscore spelled differently, and an empty
    // string. The modelled `AddressType` enum admits only `ADDRESS_TYPE_VALUES`.
    for value in ["AWS_SHIPPING", "aws_ship", "CUST PICKUP", ""] {
        let case = format!("Type {value:?}");
        assert!(
            !ADDRESS_TYPE_VALUES.contains(&value),
            "case {case:?}: {value:?} is a value the modelled AddressType admits, so it \
             does not belong in this table"
        );
        let address = deliverable_address(
            "Typed Wrong",
            "15 Enum Street",
            json!({ "Type": value }),
        );
        let (status, body) = create_in_region(&router, HOME_REGION, &address).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 for a type outside the catalog, got {status} \
             with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .contains("ValidationException"),
            "case {case:?}: expected __type ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn create_address_refuses_an_address_missing_a_member_a_delivery_needs() {
    let router = router();

    // The complete address is the control row: every member present, so the refusals
    // below are about the one member each row takes away.
    let complete = deliverable_address("Complete Receiver", "10 Whole Street", json!({}));
    let created = ok(
        &router,
        "CreateAddress",
        json!({ "Address": complete }),
        "every required member present",
    )
    .await;
    assert_prefixed_uuid(
        &string(&created, "AddressId", "every required member present"),
        "ADID",
        "every required member present",
    );

    // One row per member the emulator requires for a delivery, times the two ways a
    // request can fail to carry it: leaving it out, and sending whitespace. The set is
    // read from the crate (`REQUIRED_DELIVERY_MEMBERS`), so the table cannot drift from
    // the rule it tests — `Street1` is the member the recorded request dropped, and the
    // others (`Name`, `City`, `PostalCode`, `Country`, `PhoneNumber`) are checked here
    // for the first time.
    for (member, named_as) in REQUIRED_DELIVERY_MEMBERS {
        for (how, value) in [("omitted", None), ("whitespace only", Some("   "))] {
            let case = format!("{member} {how}");
            let mut address =
                deliverable_address("Incomplete Receiver", "11 Partial Way", json!({}));
            match value {
                None => {
                    address
                        .as_object_mut()
                        .expect("the address is a JSON object")
                        .remove(*member);
                }
                Some(blank) => address[*member] = json!(blank),
            }

            let (status, body) = create_in_region(&router, HOME_REGION, &address).await;
            assert_refused(
                status,
                &body,
                "InvalidAddressException",
                "Message",
                named_as,
                &case,
            );
        }
    }
}

/// Every serviceability row: a Region, the `Country` an address names, and whether a Snow
/// device can be delivered there.
///
/// Source: AWS publishes Snow Family availability per Region, and a device ships from the
/// Region that took the order, so a Region delivers inside its own country — the European
/// Regions sharing one area over the EU/EEA, the United Kingdom and Switzerland. A
/// destination the Region's area does not cover is not deliverable, whether it is a
/// country served from another Region or a value that names no country; a Region the
/// emulator has no area for refuses nothing.
fn serviceability_rows() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        // The Region's own country, written the ways ISO 3166-1 spells it: the alpha-2
        // code, the alpha-3 code and the English name, in any case.
        ("us-west-2", "US", true),
        ("us-east-1", "USA", true),
        ("us-west-2", "United States Of America", true),
        ("ca-central-1", "CAN", true),
        ("ap-northeast-1", "Japan", true),
        ("sa-east-1", "BR", true),
        // A European Region ships across the shared European area, not only to the
        // country it sits in.
        ("eu-west-1", "IE", true),
        ("eu-west-1", "DE", true),
        ("eu-central-1", "GB", true),
        ("eu-north-1", "CH", true),
        // Outside the Region's area: another continent, and a neighbour.
        ("us-west-2", "JP", false),
        ("us-west-2", "Japan", false),
        ("us-west-2", "DEU", false),
        ("us-east-1", "CA", false),
        ("ca-central-1", "US", false),
        ("eu-west-1", "US", false),
        ("ap-northeast-1", "KR", false),
        ("sa-east-1", "AR", false),
        // A value the Region's area cannot be matched to at all is just as undeliverable
        // from there as a country served from somewhere else.
        ("us-west-2", "ZZ", false),
        ("us-west-2", "Nowhereland", false),
        // A Region with no recorded shipping area: the emulator has nothing to measure
        // the destination against, so it accepts the address rather than inventing a
        // refusal.
        ("xx-nowhere-9", "JP", true),
        ("xx-nowhere-9", "Nowhereland", true),
    ]
}

#[tokio::test]
async fn create_address_refuses_a_country_the_requests_region_does_not_ship_to() {
    let router = router();

    for (region, country, deliverable) in serviceability_rows() {
        let case = format!("{region} -> {country}");
        let address = deliverable_address(
            "Area Receiver",
            "12 Area Street",
            json!({ "Country": country }),
        );
        let (status, body) = create_in_region(&router, region, &address).await;

        if deliverable {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the address to be accepted, got {status} with {body}"
            );
            let address_id = string(&body, "AddressId", &case);
            assert_prefixed_uuid(&address_id, "ADID", &case);
            // The accepted address is readable back from another Region too: an address
            // is stored by its id, not scoped to the Region that created it.
            let (read_status, described) = call_in_region(
                &router,
                "us-east-2",
                "DescribeAddress",
                json!({ "AddressId": address_id }),
            )
            .await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected the stored address to be readable, got \
                 {read_status} with {described}"
            );
            assert_eq!(
                described["Address"]["Country"].as_str(),
                Some(country),
                "case {case:?}: expected the stored Country to read back, got {described}"
            );
        } else {
            // The refusal the service front-end answers with: a 400 `ValidationException`
            // whose text is in the lowercase `message` member.
            assert_refused(
                status,
                &body,
                "ValidationException",
                "message",
                "not available",
                &case,
            );
        }
    }
}

#[tokio::test]
async fn a_refused_address_is_never_stored() {
    let router = router();

    // Each row is a refusal, paired with the error code it must carry. None of them may
    // leave anything behind: the store only ever holds addresses a create accepted.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "missing the first street line",
            "InvalidAddressException",
            {
                let mut address =
                    deliverable_address("No Street", "unused", json!({}));
                address
                    .as_object_mut()
                    .expect("the address is a JSON object")
                    .remove("Street1");
                address
            },
        ),
        (
            "a country this region does not ship to",
            "ValidationException",
            deliverable_address("Far Away", "13 Distant Road", json!({ "Country": "JP" })),
        ),
    ];

    for (case, error_code, address) in rows {
        let (status, body) = create_in_region(&router, HOME_REGION, &address).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 {error_code}, got {status} with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .unwrap_or_default()
                .contains(error_code),
            "case {case:?}: expected __type {error_code}, got {body}"
        );
    }

    // A later, well-formed create still works and round-trips, so neither refusal left
    // the store in a state that breaks the next caller.
    let accepted = deliverable_address("After Refusals", "14 Fine Way", json!({}));
    let address_id = create_address(&router, accepted.clone(), "after the refusals").await;
    let described = ok(
        &router,
        "DescribeAddress",
        json!({ "AddressId": address_id }),
        "after the refusals",
    )
    .await;
    assert_address_matches(
        &described["Address"],
        &accepted,
        &address_id,
        "after the refusals",
    );
}
