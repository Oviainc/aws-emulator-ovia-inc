// SPDX-License-Identifier: CC0-1.0

//! `DescribeLocations`: the Direct Connect location catalog, end to end over the
//! wire.
//!
//! Every assertion goes through `POST /` with
//! `X-Amz-Target: OvertureService.DescribeLocations` (awsJson1_1), the same path a
//! client takes, and every expectation is about what the SERVICE HOLDS: the catalog
//! seeded into this service's state.
//!
//! The rule under test is not "this account's catalog": the rows AWS publishes are
//! Region/account facility data. What is asserted is the rule the operation follows —
//! 200 with a `locations` list of `Location` structures, one per catalog location in
//! the Region the request is signed for, each rendered from the stored record, with
//! `availableMacSecPortSpeeds` the location's own speeds narrowed to the speeds
//! MACsec is offered on and `availableBillingModes` carrying the usage-based mode
//! plus the location's flat-rate mode and the Regions that mode covers.
//!
//! The table spans the class the rule generalises over: every Region the catalog
//! holds (and one it does not), port-speed sets with and without 100G/400G,
//! locations with and without MACsec, and locations with and without a flat-rate
//! billing mode.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_directconnect_overtureservice::state::{
    built_in_location_catalog, LocationRecord, ComAmazonawsDirectconnectOvertureserviceState,
    DEDICATED_PORT_SPEEDS, MAC_SEC_PORT_SPEEDS, USAGE_BILLING_MODE,
};

use crate::connection_and_virtual_interface_lifecycle::Emulator;

/// The `locations` list `DescribeLocations` answers, with the response status.
async fn describe_locations(dx: &Emulator) -> (u16, Vec<Value>) {
    // The operation's input is `smithy.api#Unit`: an empty payload is the whole
    // request.
    let (status, response) = dx.call("DescribeLocations", json!({})).await;
    (
        status,
        response["locations"].as_array().cloned().unwrap_or_default(),
    )
}

/// The `Location` JSON a catalog record must render to, computed here from the
/// record's own fields by the rules the handler has to follow — the Regions a
/// flat-rate mode covers are read back off `catalog`, so the expectation is derived
/// independently of the service's renderer.
fn expected_location(
    catalog: &BTreeMap<String, LocationRecord>,
    record: &LocationRecord,
) -> Value {
    let port_speeds: Vec<&str> = record
        .available_port_speeds
        .iter()
        .map(String::as_str)
        .collect();
    let mac_sec_speeds: Vec<&str> = if record.offers_mac_sec {
        port_speeds
            .iter()
            .filter(|speed| MAC_SEC_PORT_SPEEDS.contains(*speed))
            .copied()
            .collect()
    } else {
        Vec::new()
    };

    let mut billing_modes = vec![json!({
        "billingMode": USAGE_BILLING_MODE,
        "availablePortSpeeds": port_speeds,
    })];
    if let Some(flat_rate) = &record.flat_rate_billing_mode {
        let included: BTreeSet<&str> = catalog
            .values()
            .filter(|other| other.flat_rate_billing_mode.as_deref() == Some(flat_rate.as_str()))
            .map(|other| other.region.as_str())
            .collect();
        billing_modes.push(json!({
            "billingMode": flat_rate,
            "availablePortSpeeds": port_speeds,
            "includedRegions": included.into_iter().collect::<Vec<&str>>(),
        }));
    }

    json!({
        "locationCode": record.location_code,
        "locationName": record.location_name,
        "region": record.region,
        "availablePortSpeeds": port_speeds,
        "availableProviders": record.available_providers,
        "availableMacSecPortSpeeds": mac_sec_speeds,
        "availableBillingModes": billing_modes,
    })
}

#[tokio::test]
async fn every_region_answers_its_own_catalog_locations_in_full() {
    let catalog = built_in_location_catalog();
    let regions: BTreeSet<&str> = catalog
        .values()
        .map(|location| location.region.as_str())
        .collect();
    assert!(
        regions.len() > 1,
        "the catalog must span more than one Region for this rule to be observable, got {regions:?}"
    );

    // Each Region the catalog holds a location in, plus one it holds none in: the
    // operation is scoped to "the current Amazon Web Services Region".
    let mut cases: Vec<&str> = regions.iter().copied().collect();
    cases.push("eu-north-1");

    for case in cases {
        let dx = Emulator::in_region(case);
        let (status, locations) = describe_locations(&dx).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {locations:?}"
        );

        let expected: Vec<Value> = catalog
            .values()
            .filter(|location| location.region == case)
            .map(|location| expected_location(&catalog, location))
            .collect();
        assert_eq!(
            locations, expected,
            "case {case:?}: expected the Region's catalog locations, got {locations:?}"
        );
        // A Region with no location is an empty list on 200, not an error.
        if case == "eu-north-1" {
            assert!(
                locations.is_empty(),
                "case {case:?}: a Region the catalog holds no location in answers an empty list, got {locations:?}"
            );
        } else {
            assert!(
                !locations.is_empty(),
                "case {case:?}: the catalog holds a location in this Region, got {locations:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_location_the_issue_did_not_cite_renders_every_modeled_member() {
    // One catalog entry spelled out, rather than derived: ap-northeast-1's Tokyo
    // location, which no recorded trace names. Its flat-rate mode covers the
    // Regions the catalog offers that same mode in.
    let dx = Emulator::in_region("ap-northeast-1");
    let (status, locations) = describe_locations(&dx).await;
    assert_eq!(
        status, 200,
        "expected 200, got {status} with {locations:?}"
    );
    assert_eq!(
        locations,
        vec![json!({
            "locationCode": "EqTY2",
            "locationName": "Equinix TY2, Tokyo, JP",
            "region": "ap-northeast-1",
            "availablePortSpeeds": ["1G", "10G", "100G"],
            "availableProviders": ["Equinix", "Megaport", "NTT"],
            "availableMacSecPortSpeeds": ["10G", "100G"],
            "availableBillingModes": [
                {"billingMode": "PayAsYouGo", "availablePortSpeeds": ["1G", "10G", "100G"]},
                {
                    "billingMode": "FlatRateTier3",
                    "availablePortSpeeds": ["1G", "10G", "100G"],
                    "includedRegions": ["ap-northeast-1", "ap-southeast-1"],
                },
            ],
        })],
        "expected the Tokyo catalog entry in full, got {locations:?}"
    );
}

#[tokio::test]
async fn every_answered_location_satisfies_the_catalogs_invariants() {
    let catalog = built_in_location_catalog();
    let regions: BTreeSet<&str> = catalog
        .values()
        .map(|location| location.region.as_str())
        .collect();

    for region in regions {
        let dx = Emulator::in_region(region);
        let (status, locations) = describe_locations(&dx).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} with {locations:?}"
        );

        for location in &locations {
            let case = location["locationCode"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                location["region"],
                json!(region),
                "case {case:?}: a location answered in {region} is a location OF {region}, got {:?}",
                location["region"]
            );

            let speeds: Vec<&str> = location["availablePortSpeeds"]
                .as_array()
                .map(|speeds| speeds.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert!(
                !speeds.is_empty()
                    && speeds.iter().all(|speed| DEDICATED_PORT_SPEEDS.contains(speed)),
                "case {case:?}: every port speed is a dedicated-connection speed, got {speeds:?}"
            );
            // Ascending, in the order `DEDICATED_PORT_SPEEDS` names them: the list
            // is answered in a stable order, not a per-process-random one.
            let order: Vec<usize> = speeds
                .iter()
                .filter_map(|speed| DEDICATED_PORT_SPEEDS.iter().position(|known| known == speed))
                .collect();
            assert!(
                order.windows(2).all(|pair| pair[0] < pair[1]),
                "case {case:?}: port speeds must ascend, got {speeds:?}"
            );

            let mac_sec: Vec<&str> = location["availableMacSecPortSpeeds"]
                .as_array()
                .map(|speeds| speeds.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert!(
                mac_sec
                    .iter()
                    .all(|speed| speeds.contains(speed) && MAC_SEC_PORT_SPEEDS.contains(speed)),
                "case {case:?}: MACsec speeds are the location's own speeds narrowed to {MAC_SEC_PORT_SPEEDS:?}, got {mac_sec:?} of {speeds:?}"
            );

            let billing_modes = location["availableBillingModes"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                billing_modes
                    .first()
                    .map(|mode| mode["billingMode"].clone())
                    .unwrap_or(Value::Null),
                json!(USAGE_BILLING_MODE),
                "case {case:?}: every location offers the usage-based mode, got {billing_modes:?}"
            );
            for mode in &billing_modes {
                let mode_speeds: Vec<&str> = mode["availablePortSpeeds"]
                    .as_array()
                    .map(|speeds| speeds.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                assert!(
                    !mode_speeds.is_empty() && mode_speeds.iter().all(|speed| speeds.contains(speed)),
                    "case {case:?}: a billing mode offers only the location's own port speeds, got {mode_speeds:?} of {speeds:?}"
                );
                let included: Vec<&str> = mode["includedRegions"]
                    .as_array()
                    .map(|regions| regions.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                if mode["billingMode"] == json!(USAGE_BILLING_MODE) {
                    assert!(
                        included.is_empty(),
                        "case {case:?}: the usage-based mode includes no other Region, got {included:?}"
                    );
                } else {
                    assert!(
                        included.contains(&region),
                        "case {case:?}: a flat-rate mode offered here includes this Region, got {included:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn describing_locations_changes_nothing_a_later_caller_sees() {
    let dx = Emulator::in_region("us-west-2");
    let (status, first) = describe_locations(&dx).await;
    assert_eq!(status, 200, "expected 200, got {status} with {first:?}");

    // A create at one of those locations, then the same read again: the catalog is
    // facility data, so neither the read nor the create touches it.
    let code = first[0]["locationCode"].as_str().unwrap_or_default().to_string();
    let (create_status, created) = dx
        .call(
            "CreateConnection",
            json!({"connectionName": "catalog-read", "location": &code, "bandwidth": "1Gbps"}),
        )
        .await;
    assert_eq!(
        create_status, 200,
        "setup: expected CreateConnection at {code:?} to succeed, got {create_status} with {created}"
    );

    let (status, again) = describe_locations(&dx).await;
    assert_eq!(status, 200, "expected 200, got {status} with {again:?}");
    assert_eq!(
        again, first,
        "case {code:?}: a second read must answer the same catalog, got {again:?}"
    );
}

#[tokio::test]
async fn an_empty_catalog_answers_200_with_an_empty_list() {
    let dx = Emulator::in_region("us-east-1");
    // No operation empties the catalog, so the store is emptied directly through the
    // shared engine — the same store the handler reads.
    dx.engine()
        .state::<ComAmazonawsDirectconnectOvertureserviceState>()
        .lock()
        .locations
        .clear();

    let (status, locations) = describe_locations(&dx).await;
    assert_eq!(
        status, 200,
        "an empty catalog is still a 200, got {status} with {locations:?}"
    );
    assert!(
        locations.is_empty(),
        "an empty catalog answers an empty list, got {locations:?}"
    );
}
