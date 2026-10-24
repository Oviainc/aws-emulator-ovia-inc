// SPDX-License-Identifier: CC0-1.0

//! `DescribeLocations`: the Direct Connect location catalog, end to end over the
//! wire.
//!
//! Every assertion goes through `POST /` with
//! `X-Amz-Target: OvertureService.DescribeLocations` (awsJson1_1), the same path a
//! client takes, and every expectation is about what the SERVICE HOLDS: the catalog
//! seeded into this service's state.
//!
//! The rule under test: `DescribeLocations` answers 200 with one `Location` per
//! (colocation facility, colocation partner) pair in the Region the request is signed
//! for, where
//!
//! * `locationCode` is the facility's code suffixed with the partner's
//!   (`EqSe2` + `WBE` -> `EqSe2-WBE`) — the value a client then passes as
//!   `CreateConnection`'s `location`;
//! * `locationName` names the building AND the partner's site, joined by `" | "`,
//!   which is what `Location$locationName` means by "the name of the colocation
//!   partner and the physical site of the building";
//! * `availablePortSpeeds` is THAT location's own subset of the dedicated-connection
//!   speeds, so the list differs between locations;
//! * `availableMacSecPortSpeeds` is that same list narrowed to the speeds MACsec is
//!   offered on, or empty where it is not offered;
//! * `availableBillingModes` holds exactly one entry, the usage-based mode, on the
//!   location's own port speeds and with no `includedRegions`.
//!
//! The tables span the class the rule generalises over: every Region the catalog
//! holds (and one it holds none in), facilities with one partner and with several,
//! port-speed sets with and without 100G and 400G, and demarcations with and without
//! MACsec.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_directconnect_overtureservice::state::{
    built_in_location_catalog, catalog_port_speeds, ComAmazonawsDirectconnectOvertureserviceState,
    LocationRecord, DEDICATED_PORT_SPEEDS, DIRECT_CONNECT_PROVIDERS, MAC_SEC_PORT_SPEEDS,
    USAGE_BILLING_MODE,
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
/// record's own fields by the rules the handler has to follow, so the expectation is
/// derived independently of the service's renderer.
fn expected_location(record: &LocationRecord) -> Value {
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

    json!({
        "locationCode": record.location_code,
        "locationName": record.location_name,
        "region": record.region,
        "availablePortSpeeds": port_speeds,
        "availableProviders": record.available_providers,
        "availableMacSecPortSpeeds": mac_sec_speeds,
        // Exactly one mode, on the location's own speeds, with no `includedRegions`.
        "availableBillingModes": [{
            "billingMode": USAGE_BILLING_MODE,
            "availablePortSpeeds": port_speeds,
        }],
    })
}

/// The catalog's locations grouped by facility code: the half of `locationCode`
/// before the partner suffix.
fn facilities(catalog: &BTreeMap<String, LocationRecord>) -> BTreeMap<String, Vec<&LocationRecord>> {
    let mut grouped: BTreeMap<String, Vec<&LocationRecord>> = BTreeMap::new();
    for record in catalog.values() {
        let (facility, _) = record
            .location_code
            .split_once('-')
            .unwrap_or((record.location_code.as_str(), ""));
        grouped
            .entry(facility.to_string())
            .or_default()
            .push(record);
    }
    grouped
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
            .map(expected_location)
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
async fn a_location_is_published_per_colocation_partner_at_a_facility() {
    let catalog = built_in_location_catalog();
    let grouped = facilities(&catalog);

    // The rule is only observable where one facility carries several partners, and
    // AWS publishes such facilities (an AWS device in one building, cross connects
    // delivered at more than one partner's patch panel).
    assert!(
        grouped.values().any(|rows| rows.len() > 1),
        "the catalog must hold a facility with several colocation partners, got {:?}",
        grouped
            .iter()
            .map(|(facility, rows)| (facility, rows.len()))
            .collect::<Vec<_>>()
    );

    for (facility, rows) in &grouped {
        let mut partner_codes: Vec<&str> = Vec::new();
        let mut partner_sites: Vec<&str> = Vec::new();
        for record in rows {
            let case = record.location_code.as_str();
            let (code_facility, partner_code) = record
                .location_code
                .split_once('-')
                .unwrap_or((case, ""));
            assert_eq!(
                code_facility, facility,
                "case {case:?}: a location code starts with its facility's code, got {code_facility:?}"
            );
            assert!(
                !partner_code.is_empty(),
                "case {case:?}: a location code carries the colocation partner's code after the facility's, got {case:?}"
            );

            let (building, partner_site) = record
                .location_name
                .split_once(" | ")
                .unwrap_or((record.location_name.as_str(), ""));
            assert!(
                !building.is_empty() && !partner_site.is_empty(),
                "case {case:?}: a location name names the building AND the colocation partner's site, got {:?}",
                record.location_name
            );
            // Every location at one facility is in the SAME building, and names it
            // identically.
            let first_building = rows[0]
                .location_name
                .split_once(" | ")
                .map(|(building, _)| building)
                .unwrap_or_default();
            assert_eq!(
                building, first_building,
                "case {case:?}: every location at {facility:?} names the same building, got {building:?}"
            );
            assert_eq!(
                record.region, rows[0].region,
                "case {case:?}: every location at {facility:?} is in one Region, got {:?}",
                record.region
            );

            partner_codes.push(partner_code);
            partner_sites.push(partner_site);
        }

        // One location per partner: no partner is published twice at a facility, and
        // no two of its locations name the same partner site.
        let unique_codes: BTreeSet<&str> = partner_codes.iter().copied().collect();
        assert_eq!(
            unique_codes.len(),
            partner_codes.len(),
            "case {facility:?}: each colocation partner is published once, got {partner_codes:?}"
        );
        let unique_sites: BTreeSet<&str> = partner_sites.iter().copied().collect();
        assert_eq!(
            unique_sites.len(),
            partner_sites.len(),
            "case {facility:?}: each of a facility's locations names its own partner site, got {partner_sites:?}"
        );
        // A bare facility code is not a location a client can order at.
        assert!(
            !catalog.contains_key(facility.as_str()),
            "case {facility:?}: a facility code alone is not a published location code"
        );
    }
}

#[tokio::test]
async fn a_location_the_issue_did_not_cite_renders_every_modeled_member() {
    // Two catalog entries spelled out rather than derived: eu-central-1's Frankfurt
    // facility, which no recorded trace names. Both its partner demarcations, one
    // with MACsec on its own 10G/100G and one with none, and each with the single
    // usage-based billing mode.
    //
    // Sources: `Location` (locationCode/locationName/region/availablePortSpeeds/
    // availableProviders/availableMacSecPortSpeeds/availableBillingModes, via
    // smithy_query shape="Location"), the AWS Direct Connect facility and
    // delivery-partner lists for the provider names and sites.
    let dx = Emulator::in_region("eu-central-1");
    let (status, locations) = describe_locations(&dx).await;
    assert_eq!(status, 200, "expected 200, got {status} with {locations:?}");

    let providers = json!([
        "Arelion",
        "BT",
        "Colt Technology Services",
        "Equinix, Inc.",
        "GTT Communications",
        "Megaport",
        "Orange Business Services",
        "Vodafone",
    ]);
    assert_eq!(
        locations,
        vec![
            json!({
                "locationCode": "EqFR5-EQ",
                "locationName": "Equinix FR5, Frankfurt, DE | Equinix, FR5, Frankfurt, DE",
                "region": "eu-central-1",
                "availablePortSpeeds": ["100G", "1G", "10G"],
                "availableProviders": providers,
                "availableMacSecPortSpeeds": ["100G", "10G"],
                "availableBillingModes": [
                    {"billingMode": "PayAsYouGo", "availablePortSpeeds": ["100G", "1G", "10G"]},
                ],
            }),
            json!({
                "locationCode": "EqFR5-ITX",
                "locationName": "Equinix FR5, Frankfurt, DE | Interxion, FRA6, Frankfurt, DE",
                "region": "eu-central-1",
                "availablePortSpeeds": ["1G", "10G"],
                "availableProviders": providers,
                "availableMacSecPortSpeeds": [],
                "availableBillingModes": [
                    {"billingMode": "PayAsYouGo", "availablePortSpeeds": ["1G", "10G"]},
                ],
            }),
        ],
        "expected the Frankfurt facility's two locations in full, got {locations:?}"
    );
}

#[tokio::test]
async fn every_answered_location_satisfies_the_catalogs_invariants() {
    let catalog = built_in_location_catalog();
    let regions: BTreeSet<&str> = catalog
        .values()
        .map(|location| location.region.as_str())
        .collect();
    let known_providers: BTreeSet<&str> = DIRECT_CONNECT_PROVIDERS.iter().copied().collect();
    let mut speed_sets: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut mac_sec_speeds_seen: BTreeSet<String> = BTreeSet::new();

    for region in regions {
        let dx = Emulator::in_region(region);
        let (status, locations) = describe_locations(&dx).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} with {locations:?}"
        );

        for location in &locations {
            let case = location["locationCode"]
                .as_str()
                .unwrap_or_default()
                .to_string();
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
                    && speeds
                        .iter()
                        .all(|speed| DEDICATED_PORT_SPEEDS.contains(speed)),
                "case {case:?}: every port speed is a dedicated-connection speed, got {speeds:?}"
            );
            // Stable and repeat-free: `catalog_port_speeds` publishes them in
            // `DEDICATED_PORT_SPEEDS` order, which is the order the recorded response
            // lists them in.
            let order: Vec<usize> = speeds
                .iter()
                .filter_map(|speed| {
                    DEDICATED_PORT_SPEEDS
                        .iter()
                        .position(|known| known == speed)
                })
                .collect();
            assert!(
                order.windows(2).all(|pair| pair[0] < pair[1]),
                "case {case:?}: port speeds are published in {DEDICATED_PORT_SPEEDS:?} order without repeats, got {speeds:?}"
            );
            speed_sets.insert(speeds.iter().map(ToString::to_string).collect());

            let mac_sec: Vec<&str> = location["availableMacSecPortSpeeds"]
                .as_array()
                .map(|speeds| speeds.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let narrowed: Vec<&str> = speeds
                .iter()
                .filter(|speed| MAC_SEC_PORT_SPEEDS.contains(*speed))
                .copied()
                .collect();
            assert!(
                mac_sec.is_empty() || mac_sec == narrowed,
                "case {case:?}: MACsec speeds are the location's own speeds narrowed to {MAC_SEC_PORT_SPEEDS:?} ({narrowed:?}) or none, got {mac_sec:?}"
            );
            mac_sec_speeds_seen.extend(mac_sec.iter().map(ToString::to_string));

            let providers: Vec<&str> = location["availableProviders"]
                .as_array()
                .map(|providers| providers.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert!(
                !providers.is_empty(),
                "case {case:?}: a location names the service providers reachable there, got {providers:?}"
            );
            let unique: BTreeSet<&str> = providers.iter().copied().collect();
            assert_eq!(
                unique.len(),
                providers.len(),
                "case {case:?}: no provider is named twice, got {providers:?}"
            );
            assert!(
                unique.iter().all(|provider| known_providers.contains(provider)),
                "case {case:?}: every provider is a Direct Connect delivery partner named as AWS reports it, got {providers:?}"
            );

            let billing_modes = location["availableBillingModes"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                billing_modes,
                vec![json!({
                    "billingMode": USAGE_BILLING_MODE,
                    "availablePortSpeeds": speeds,
                })],
                "case {case:?}: a location publishes exactly the usage-based mode, on its own port speeds and with no includedRegions, got {billing_modes:?}"
            );
        }
    }

    // Per-facility, not catalog-wide: the answers must not all carry one uniform
    // port-speed list, and 400G must be observable as something only some
    // demarcations offer.
    assert!(
        speed_sets.len() > 2,
        "port speeds are per-location data, so the catalog must publish several different lists, got {speed_sets:?}"
    );
    assert!(
        speed_sets.iter().any(|speeds| speeds.iter().any(|speed| speed == "400G"))
            && speed_sets
                .iter()
                .any(|speeds| speeds.iter().all(|speed| speed != "400G")),
        "400G is offered at some facilities and not others, got {speed_sets:?}"
    );
    assert!(
        mac_sec_speeds_seen.contains("400G"),
        "a 400G demarcation that offers MACsec publishes 400G among its MACsec speeds, got {mac_sec_speeds_seen:?}"
    );
}

#[test]
fn published_port_speeds_are_the_known_speeds_in_one_stable_order() {
    // The derivation every location's `availablePortSpeeds` is built by (and, through
    // it, its `availableMacSecPortSpeeds` and its billing mode's speeds). The rows
    // differ along what the rule generalises over: which speeds, how many, the order
    // they are written in, repeats, and a speed no dedicated connection offers.
    // Source for the speed set: `CreateLagRequest$connectionsBandwidth` ("1Gbps,
    // 10Gbps, 100Gbps, and 400Gbps"), in `PortSpeed` short form.
    let rows: &[(&str, &[&str], &[&str])] = &[
        ("single", &["1G"], &["1G"]),
        ("no-100g", &["1G", "10G"], &["1G", "10G"]),
        ("with-100g", &["1G", "10G", "100G"], &["100G", "1G", "10G"]),
        (
            "with-400g",
            &["1G", "10G", "100G", "400G"],
            &["100G", "1G", "10G", "400G"],
        ),
        ("written-descending", &["400G", "100G"], &["100G", "400G"]),
        ("repeated", &["10G", "10G", "1G"], &["1G", "10G"]),
        ("fastest-only", &["400G"], &["400G"]),
        ("unknown-dropped", &["1G", "40G", "2.5G"], &["1G"]),
        ("all-unknown", &["40G"], &[]),
        ("empty", &[], &[]),
    ];

    for (case, written, expected) in rows {
        let published = catalog_port_speeds(written);
        assert_eq!(
            published, *expected,
            "case {case:?}: expected {expected:?} from {written:?}, got {published:?}"
        );
    }
}

#[tokio::test]
async fn describing_locations_changes_nothing_a_later_caller_sees() {
    let dx = Emulator::in_region("us-west-2");
    let (status, first) = describe_locations(&dx).await;
    assert_eq!(status, 200, "expected 200, got {status} with {first:?}");

    // A create at one of those locations, then the same read again: the catalog is
    // facility data, so neither the read nor the create touches it.
    let code = first[0]["locationCode"]
        .as_str()
        .unwrap_or_default()
        .to_string();
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
