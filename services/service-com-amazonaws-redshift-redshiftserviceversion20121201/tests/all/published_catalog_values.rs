// SPDX-License-Identifier: CC0-1.0

//! The VALUE VOCABULARIES and structure shapes the account-independent catalog
//! reads publish, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle and
//! reserved-node suites own rather than keeping a second copy of either.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request:
//!
//! * a published reserved node offering is published under one of the service's
//!   PAYMENT OPTIONS, is a `Regular` offering (the purchase catalog is bought
//!   outright), charges nothing per running hour, and carries its term's
//!   unpaid remainder as ONE hourly `RecurringCharges` STRUCTURE — on every
//!   entry, so a client reading `RecurringCharges.RecurringCharge` finds a
//!   charge rather than an empty member;
//! * the offerings an existing reservation may be exchanged ONTO are a catalog
//!   of their own: `Upgradable` offerings of the exchange-target node family,
//!   whose identifiers are none of the purchase catalog's;
//! * the two maintenance tracks apply DIFFERENT engine releases — `current` the
//!   most recently certified one, `trailing` the one certified before it — and
//!   every `UpdateTarget` reports the release of the track it points at.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, element_blocks, Emulator};
use crate::reserved_nodes::{catalog, offering_of, one, poll_until_state, purchase, ONE_YEAR, THREE_YEARS};

/// Every payment option the reserved-node catalog is published under, as
/// `(OfferingType, share of the term charged upfront)`, written out here rather
/// than read off the production table so a wrong entry fails this suite.
///
/// Source: the payment options Amazon Redshift publishes reserved nodes under.
/// The utilization classes reserved nodes were once published under are retired
/// and must appear nowhere in the served catalog, which
/// [`RETIRED_OFFERING_TYPES`] pins.
const PAYMENT_OPTIONS: [(&str, f64); 3] = [
    ("No Upfront", 0.0),
    ("Partial Upfront", 0.5),
    ("All Upfront", 1.0),
];

/// The offering types Amazon Redshift retired: no served entry may carry one.
const RETIRED_OFFERING_TYPES: [&str; 3] = [
    "Light Utilization",
    "Medium Utilization",
    "Heavy Utilization",
];

/// The frequency the unpaid remainder of a term is charged at.
///
/// Source: `smithy_query(query="shape", shape="RecurringCharge")` —
/// `RecurringChargeFrequency` is "The frequency at which the recurring charge
/// amount is applied", and the catalog applies it per hour of the term.
const HOURLY: &str = "Hourly";

/// The `ReservedNodeOfferingType` every entry of the PURCHASE catalog carries.
///
/// Source: `smithy_query(query="shape", shape="ReservedNodeOfferingType")` — the
/// enum's two values are `Regular` and `Upgradable`.
const REGULAR: &str = "Regular";

/// The `ReservedNodeOfferingType` every entry of the EXCHANGE-TARGET catalog
/// carries, from the same enum.
const UPGRADABLE: &str = "Upgradable";

/// The node-type family an exchange can target.
///
/// Source: `smithy_query(query="doc", shape="GetReservedNodeExchangeOfferings")`
/// — "Returns an array of DC2 ReservedNodeOfferings that matches the payment
/// type, term, and usage price of the given DC1 reserved node".
const EXCHANGE_TARGET_FAMILY: &str = "dc2.";

/// How closely a served amount must match the rule, in currency units per hour.
const PRICE_TOLERANCE: f64 = 1e-3;

/// The FIRST `<tag>` of one record: a maintenance track nests its update
/// targets, which carry the same member names, and a structure serializes its
/// members in declaration order — so a track's own name and release are the
/// first of each inside its element.
fn first(case: &str, block: &str, tag: &str) -> String {
    all_text(block, tag)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case}: expected a <{tag}> in {block}"))
}

/// One served offering's `<tag>` as a number.
fn number(case: &str, block: &str, tag: &str) -> f64 {
    let text = one(case, block, tag);
    text.parse::<f64>()
        .unwrap_or_else(|_| panic!("case {case}: <{tag}> must be a number, got {text:?}"))
}

/// Every `RecurringCharge` element of one offering, as `(amount, frequency)`.
fn recurring_charges(case: &str, block: &str) -> Vec<(f64, String)> {
    element_blocks(block, "RecurringCharge")
        .into_iter()
        .map(|charge| {
            (
                number(case, &charge, "RecurringChargeAmount"),
                one(case, &charge, "RecurringChargeFrequency"),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_published_offering_carries_a_payment_option_and_an_hourly_recurring_charge() {
    let emulator = Emulator::new();
    let served = catalog(&emulator).await;

    // Rows span the dimensions the rule generalises over: the three node-type
    // families the catalog offers, both published terms and all three payment
    // options — including entries no report cited.
    let rows: [(&str, &str, &str, &str); 8] = [
        ("the smallest dense-compute node, one year, nothing upfront", "dc2.large", ONE_YEAR, "No Upfront"),
        ("the largest dense-compute node, three years, all upfront", "dc2.8xlarge", THREE_YEARS, "All Upfront"),
        ("a reserve-group node, one year, part upfront", "rg.large", ONE_YEAR, "Partial Upfront"),
        ("the largest reserve-group node, three years, nothing upfront", "rg.12xlarge", THREE_YEARS, "No Upfront"),
        ("a mid-sized reserve-group node, one year, all upfront", "rg.xlarge", ONE_YEAR, "All Upfront"),
        ("a managed-storage node, three years, part upfront", "ra3.4xlarge", THREE_YEARS, "Partial Upfront"),
        ("the largest managed-storage node, one year, part upfront", "ra3.16xlarge", ONE_YEAR, "Partial Upfront"),
        ("a mid-sized managed-storage node, three years, all upfront", "ra3.xlplus", THREE_YEARS, "All Upfront"),
    ];

    for (case, node_type, duration, offering_type) in rows {
        let block = offering_of(case, &served, node_type, duration, offering_type);

        // The payment option is one the service publishes, and the entry is a
        // product a purchase buys outright.
        assert!(
            PAYMENT_OPTIONS
                .iter()
                .any(|(published, _)| *published == offering_type),
            "case {case}: expected a payment option from {PAYMENT_OPTIONS:?}, got {offering_type:?}"
        );
        assert_eq!(
            one(case, &block, "ReservedNodeOfferingType"),
            REGULAR,
            "case {case}: expected the purchase catalog to publish a {REGULAR} offering, got {block}"
        );

        // Nothing is charged per RUNNING hour: the term's unpaid remainder is a
        // recurring charge, which every entry carries as a structure.
        assert_eq!(
            number(case, &block, "UsagePrice"),
            0.0,
            "case {case}: expected nothing charged per running hour, got {block}"
        );
        let charges = recurring_charges(case, &block);
        assert_eq!(
            charges.len(),
            1,
            "case {case}: expected one RecurringCharge structure, got {charges:?} in {block}"
        );
        let (amount, frequency) = &charges[0];
        assert_eq!(
            frequency, HOURLY,
            "case {case}: expected the remainder charged {HOURLY}, got {frequency:?}"
        );

        // Upfront plus recurring is the whole term, split the way the payment
        // option says: the fully-upfront option leaves nothing recurring, and
        // every other option charges something every hour.
        let upfront = PAYMENT_OPTIONS
            .iter()
            .find(|(published, _)| *published == offering_type)
            .map(|(_, share)| *share)
            .unwrap_or_default();
        let hours = number(case, &block, "Duration") / 3600.0;
        let term_rate = number(case, &block, "FixedPrice") / hours + amount;
        assert!(
            (number(case, &block, "FixedPrice") - term_rate * hours * upfront).abs()
                <= PRICE_TOLERANCE * hours,
            "case {case}: expected {upfront} of the term charged upfront, got {block}"
        );
        if (upfront - 1.0).abs() < f64::EPSILON {
            assert_eq!(
                *amount, 0.0,
                "case {case}: paying the whole term upfront leaves nothing recurring, got {amount}"
            );
        } else {
            assert!(
                *amount > 0.0,
                "case {case}: a term not paid in full upfront is charged hourly, got {amount}"
            );
        }

        // The id names this one offering, and the by-id read reports the very
        // same entry — the catalog is one list however it is asked for.
        let offering_id = one(case, &block, "ReservedNodeOfferingId");
        let selected = emulator
            .call(
                "DescribeReservedNodeOfferings",
                &[("ReservedNodeOfferingId", offering_id.as_str())],
            )
            .await;
        assert_eq!(
            selected.status, 200,
            "case {case}: expected the by-id read to be served, got {} with body {}",
            selected.status, selected.body
        );
        assert_eq!(
            element_blocks(&selected.body, "ReservedNodeOffering"),
            vec![block.clone()],
            "case {case}: expected the by-id read to report the same entry, got body {}",
            selected.body
        );
    }

    // Over the WHOLE catalog: every payment option is published, no retired
    // utilization class is, and no entry is served with an empty
    // `RecurringCharges` member.
    let offering_types: Vec<String> = served
        .iter()
        .map(|block| one("the whole catalog", block, "OfferingType"))
        .collect();
    for (published, _) in PAYMENT_OPTIONS {
        assert!(
            offering_types.iter().any(|served| served == published),
            "case the whole catalog: expected the payment option {published:?} to be published, got {offering_types:?}"
        );
    }
    for retired in RETIRED_OFFERING_TYPES {
        assert!(
            !offering_types.iter().any(|served| served == retired),
            "case the whole catalog: expected no retired offering type, got {retired:?}"
        );
    }
    for block in &served {
        let case = format!(
            "the {} / {} offering",
            one("the whole catalog", block, "NodeType"),
            one("the whole catalog", block, "OfferingType")
        );
        assert_eq!(
            all_text(block, "RecurringChargeFrequency"),
            vec![HOURLY.to_string()],
            "case {case}: expected one hourly recurring charge on every entry, got {block}"
        );
        assert_eq!(
            one(&case, block, "ReservedNodeOfferingType"),
            REGULAR,
            "case {case}: expected every purchase-catalog entry to be {REGULAR}, got {block}"
        );
    }
}

#[tokio::test]
async fn the_offerings_an_exchange_may_target_are_a_catalog_of_their_own() {
    let emulator = Emulator::new();
    let served = catalog(&emulator).await;

    // Buy a reservation, then read what it may be exchanged onto: rows differ
    // along the term and the payment option the reservation was bought under,
    // the two dimensions an exchange keeps.
    for (case, node_type, duration, offering_type) in [
        ("a one-year part-upfront reservation", "dc2.large", ONE_YEAR, "Partial Upfront"),
        ("a three-year fully-upfront reservation", "ra3.large", THREE_YEARS, "All Upfront"),
    ] {
        let offering = offering_of(case, &served, node_type, duration, offering_type);
        let bought = purchase(&emulator, &offering, Some(1), case).await;
        assert_eq!(
            bought.status, 200,
            "case {case}: expected the purchase to be served, got {} with body {}",
            bought.status, bought.body
        );
        let reserved_node_id = bought
            .text("ReservedNodeId")
            .unwrap_or_else(|| panic!("case {case}: the purchase must report a ReservedNodeId"));
        poll_until_state(
            &emulator,
            case,
            &reserved_node_id,
            state::RESERVED_NODE_STATE_ACTIVE,
        )
        .await;

        let answer = emulator
            .call(
                "GetReservedNodeExchangeOfferings",
                &[("ReservedNodeId", reserved_node_id.as_str())],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the exchange-offering read to be served, got {} with body {}",
            answer.status, answer.body
        );
        let targets = element_blocks(&answer.body, "ReservedNodeOffering");
        assert!(
            !targets.is_empty(),
            "case {case}: expected the published exchange targets, got body {}",
            answer.body
        );

        let purchase_ids: Vec<String> = served
            .iter()
            .map(|block| one(case, block, "ReservedNodeOfferingId"))
            .collect();
        for target in &targets {
            assert_eq!(
                one(case, target, "ReservedNodeOfferingType"),
                UPGRADABLE,
                "case {case}: an offering an exchange may target is {UPGRADABLE}, got {target}"
            );
            let target_node_type = one(case, target, "NodeType");
            assert!(
                target_node_type.starts_with(EXCHANGE_TARGET_FAMILY),
                "case {case}: expected a {EXCHANGE_TARGET_FAMILY} node type, got {target_node_type:?}"
            );
            assert_eq!(
                one(case, target, "OfferingType"),
                offering_type,
                "case {case}: an exchange keeps the payment type, got {target}"
            );
            assert_eq!(
                one(case, target, "Duration"),
                duration,
                "case {case}: an exchange keeps the term, got {target}"
            );
            let target_id = one(case, target, "ReservedNodeOfferingId");
            assert!(
                !purchase_ids.contains(&target_id),
                "case {case}: an exchange target is not an entry of the purchase catalog, got {target_id}"
            );
        }
    }
}

#[tokio::test]
async fn the_two_maintenance_tracks_apply_different_engine_releases() {
    let emulator = Emulator::new();

    let answer = emulator.call("DescribeClusterTracks", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case the whole track catalog: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let tracks = element_blocks(&answer.body, "MaintenanceTrack");
    assert_eq!(
        tracks.len(),
        2,
        "case the whole track catalog: expected the two published tracks, got {tracks:?}"
    );

    // The release each track applies, read off the answer: `current` applies the
    // most recently certified maintenance release and `trailing` the one
    // certified before it, so the two are DIFFERENT releases.
    let mut releases: Vec<(String, String)> = Vec::new();
    for track in &tracks {
        let case = "a published track";
        releases.push((
            first(case, track, "MaintenanceTrackName"),
            first(case, track, "DatabaseVersion"),
        ));
    }
    assert_ne!(
        releases[0].1, releases[1].1,
        "case the whole track catalog: expected the two tracks to apply different releases, got {releases:?}"
    );

    // Every `UpdateTarget` reports the release of the track it points AT, so a
    // client diffing current against trailing reads the same version from either
    // side, and the operations a switch is performed by in the order the service
    // reports them.
    for track in &tracks {
        let case = format!(
            "the {} track",
            first("a published track", track, "MaintenanceTrackName")
        );
        for target in element_blocks(track, "UpdateTarget") {
            let name = one(&case, &target, "MaintenanceTrackName");
            let expected = releases
                .iter()
                .find(|(track_name, _)| *track_name == name)
                .map(|(_, release)| release.clone())
                .unwrap_or_else(|| panic!("case {case}: {name} is not a published track"));
            assert_eq!(
                one(&case, &target, "DatabaseVersion"),
                expected,
                "case {case}: expected the update target to report the {name} track's own release, got {target}"
            );
            assert_eq!(
                all_text(&target, "OperationName"),
                state::TRACK_SWITCH_OPERATIONS.map(str::to_string).to_vec(),
                "case {case}: expected the switch operations in the order the service reports them, got {target}"
            );
        }
    }

    // Selecting a track by name reports that track's own release, so the
    // filtered read and the unfiltered one cannot disagree.
    for (name, release) in &releases {
        let selected = emulator
            .call("DescribeClusterTracks", &[("MaintenanceTrackName", name)])
            .await;
        assert_eq!(
            selected.status, 200,
            "case select {name}: expected 200, got {} with body {}",
            selected.status, selected.body
        );
        let one_track = element_blocks(&selected.body, "MaintenanceTrack");
        assert_eq!(
            one_track.len(),
            1,
            "case select {name}: expected just the track the name selects, got {one_track:?}"
        );
        assert_eq!(
            first(name, &one_track[0], "DatabaseVersion"),
            *release,
            "case select {name}: expected the release the unfiltered read reports, got {}",
            one_track[0]
        );
    }
}
