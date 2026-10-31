// SPDX-License-Identifier: CC0-1.0

//! The CHANGE-FREE `UpdateCoreNetwork`, and what the rest of the service reads while one is
//! outstanding, driven end-to-end over HTTP through the real router.
//!
//! The rule pinned here: an update that asks a core network for the state it is ALREADY in
//! leaves the service no change to apply, so it writes nothing — it does not move the core
//! network into the non-terminal state a reconfiguration is outstanding in, and it leaves
//! every member, the ids, the ARN, `CreatedAt` and the tags exactly as the call found them.
//! A request asks for no change when it carries no mutable member at all, or carries the
//! value the record already holds. Because nothing is written, the policy in force stays
//! readable through it: the route table each of the core network's segments holds at each of
//! its edge locations is the one a caller would have read had the update never been made.
//!
//! The rows below span that class along the FORM of the value the record holds and the
//! request repeats — `Description` is a `ConstrainedString`, length 0..256 (`smithy_query
//! query="operation" operation="UpdateCoreNetwork"`), so the forms are the empty string (the
//! documented removal), a single character, a value at the modelled maximum length, a
//! multi-line and non-ASCII value, and an ordinary sentence — and the route tables are read
//! at coordinates spanning a segment present at every edge, a segment restricted to one
//! edge, a segment no action declares a route for, and both edge locations. Each row also
//! makes ONE request that DOES ask for a change, so a row cannot pass against an updater
//! that never records a transition at all.
//!
//! Nothing reaches into `crate::state`: every core network is created, updated and read back
//! through operations. The harness is the one `global_network_lifecycle` defines.

use serde_json::{json, Value};

use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{Emulator, PAST_SETTLE};

/// The state a settled core network reports, and the one a reconfiguration is outstanding in
/// (`smithy_query shape=CoreNetworkState`).
const AVAILABLE: &str = "AVAILABLE";
/// See [`AVAILABLE`].
const UPDATING: &str = "UPDATING";

/// The two Regions the subject's policy configures an edge in.
const WEST: &str = "us-west-2";
/// See [`WEST`].
const CENTRAL: &str = "eu-central-1";

/// The segments the subject's policy declares: one present at every edge, one restricted to
/// the western edge, and one no `create-route` action declares a route for.
const SHARED: &str = "shared";
/// See [`SHARED`].
const PROD: &str = "prod";
/// See [`SHARED`].
const QUIET: &str = "quiet";

/// The `Tags` every seeded core network carries, which no update may rewrite.
fn base_tags() -> Value {
    json!([{ "Key": "env", "Value": "test" }])
}

/// The policy document every seeded core network holds. Its key names
/// (`core-network-configuration`, `asn-ranges`, `edge-locations`, `segments`, `location`,
/// `asn`, `segment-actions`, `action`, `create-route`, `destination-cidr-blocks`,
/// `destinations`, `blackhole`) are the published Cloud WAN core network policy schema's,
/// the same source this crate's other policy suites build documents from.
///
/// Every route it declares is reached through nothing (`blackhole`), so the route tables it
/// configures need no attachment to be held.
fn policy_document() -> Value {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [
                { "location": WEST, "asn": 64512 },
                { "location": CENTRAL, "asn": 64513 },
            ],
        },
        "segments": [
            { "name": SHARED },
            { "name": PROD, "edge-locations": [WEST] },
            { "name": QUIET },
        ],
        "segment-actions": [
            {
                "action": "create-route",
                "segment": SHARED,
                "destination-cidr-blocks": ["10.0.0.0/16", "10.1.0.0/16"],
                "destinations": ["blackhole"],
            },
            {
                "action": "create-route",
                "segment": PROD,
                "destination-cidr-blocks": ["172.16.0.0/16"],
                "destinations": ["blackhole"],
            },
        ],
    })
}

/// One reported route that is reached through nothing: a blackholed static route carries no
/// next hop, and is originated by the core network itself, so its AS path is empty.
fn blackholed(prefix: &str) -> Value {
    json!({ "Prefix": prefix, "AsPath": [] })
}

/// One coordinate a route table is addressed by, and the routes the policy above holds
/// there.
struct Coordinate {
    case: &'static str,
    segment: &'static str,
    edge: &'static str,
    expected: Vec<Value>,
}

/// The coordinates each row reads: a segment at every edge read at both of them, a segment
/// restricted to the western edge read at both, and a segment holding no route at all.
fn coordinates() -> Vec<Coordinate> {
    vec![
        Coordinate {
            case: "the segment of every edge, at the western one",
            segment: SHARED,
            edge: WEST,
            expected: vec![blackholed("10.0.0.0/16"), blackholed("10.1.0.0/16")],
        },
        Coordinate {
            case: "the segment of every edge, at the central one",
            segment: SHARED,
            edge: CENTRAL,
            expected: vec![blackholed("10.0.0.0/16"), blackholed("10.1.0.0/16")],
        },
        Coordinate {
            case: "the segment restricted to the western edge, at that edge",
            segment: PROD,
            edge: WEST,
            expected: vec![blackholed("172.16.0.0/16")],
        },
        Coordinate {
            case: "the segment restricted to the western edge, at the other one",
            segment: PROD,
            edge: CENTRAL,
            expected: Vec::new(),
        },
        Coordinate {
            case: "the segment no action declares a route for",
            segment: QUIET,
            edge: WEST,
            expected: Vec::new(),
        },
    ]
}

/// One row of the table: the `Description` its core network is seeded with, and therefore
/// the value a request can repeat back at it.
struct DescriptionCase {
    case: &'static str,
    description: String,
}

/// The rows: the forms a `ConstrainedString` description takes.
fn description_cases() -> Vec<DescriptionCase> {
    vec![
        DescriptionCase {
            case: "an ordinary sentence",
            description: "seeded description".to_string(),
        },
        DescriptionCase {
            case: "the empty string, the documented removal",
            description: String::new(),
        },
        DescriptionCase {
            case: "a single character",
            description: "x".to_string(),
        },
        DescriptionCase {
            case: "a value at the modelled maximum length of 256 characters",
            description: "d".repeat(256),
        },
        DescriptionCase {
            case: "a multi-line and non-ascii value",
            description: "line one\nline two \u{1f310}".to_string(),
        },
    ]
}

/// Create a global network and, inside it, a core network holding `description`,
/// [`policy_document`] and [`base_tags`]. A global network holds at most one core network,
/// so every row needs its own parent.
async fn seed(emulator: &Emulator, case: &str, description: &str) -> String {
    let network = emulator.create_global_network(case).await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network,
            "Description": description,
            "PolicyDocument": policy_document().to_string(),
            "Tags": base_tags(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkId in {created}"))
        .to_string()
}

/// One core network as `GetCoreNetwork` reports it.
async fn read_core_network(emulator: &Emulator, case: &str, id: &str) -> Value {
    let (status, body) = emulator.get(&format!("/core-networks/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetwork expected 200, got {status} with {body}"
    );
    body["CoreNetwork"].clone()
}

/// The routes one segment holds at one edge location, as
/// `ListCoreNetworkRoutingInformation` reports them.
async fn routing(
    emulator: &Emulator,
    case: &str,
    id: &str,
    segment: &str,
    edge: &str,
) -> Vec<Value> {
    let (status, body) = emulator
        .post(
            &format!("/core-networks/{id}/core-network-routing-information"),
            json!({ "SegmentName": segment, "EdgeLocation": edge }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCoreNetworkRoutingInformation expected 200, got {status} with {body}"
    );
    body["CoreNetworkRoutingInformation"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a CoreNetworkRoutingInformation array in {body}")
        })
        .clone()
}

/// Every coordinate of one core network, asserted against the routes the policy in force
/// configures there.
async fn assert_route_tables(emulator: &Emulator, case: &str, id: &str) {
    for coordinate in &coordinates() {
        let reported = routing(emulator, case, id, coordinate.segment, coordinate.edge).await;
        assert_eq!(
            reported, coordinate.expected,
            "case {case:?} / {:?}: expected the routes the policy in force configures ({:?}), got {reported:?}",
            coordinate.case, coordinate.expected
        );
    }
}

#[tokio::test]
async fn a_change_free_update_writes_nothing_and_leaves_every_route_table_readable() {
    let emulator = Emulator::new();
    let rows = description_cases();

    let mut seeded = Vec::new();
    for row in &rows {
        seeded.push(seed(&emulator, row.case, &row.description).await);
    }

    // The create answers the core network already AVAILABLE, so a state anything but
    // AVAILABLE below was moved by one of the calls this suite makes.
    tokio::time::sleep(PAST_SETTLE).await;

    for (row, id) in rows.iter().zip(&seeded) {
        let case = row.case;
        let before = read_core_network(&emulator, case, id).await;
        assert_eq!(
            before["State"],
            json!(AVAILABLE),
            "case {case:?}: expected a settled core network before the updates, got {before}"
        );
        assert_route_tables(&emulator, case, id).await;

        let change_free = [
            ("no mutable member at all".to_string(), json!({})),
            (
                "the Description the record holds".to_string(),
                json!({ "Description": row.description }),
            ),
            // Repeating one of them is the same request again: a change-free update is
            // idempotent, because there is nothing for a second one to find changed either.
            ("no mutable member at all, again".to_string(), json!({})),
        ];

        for (label, body) in &change_free {
            let (status, answer) = emulator
                .patch(&format!("/core-networks/{id}"), body.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {label:?}: expected 200, got {status} with {answer}"
            );
            let core_network = &answer["CoreNetwork"];
            assert_eq!(
                core_network["State"],
                json!(AVAILABLE),
                "case {case:?} / {label:?}: a change-free update owes no transition, got {core_network}"
            );
            // The whole record, member for member: a moved state, a restamped instant or a
            // rewritten member fails here.
            assert_eq!(
                *core_network, before,
                "case {case:?} / {label:?}: expected the record untouched ({before}), got {core_network}"
            );

            // And the next caller sees the same record: nothing was written to the store.
            let reread = read_core_network(&emulator, case, id).await;
            assert_eq!(
                reread, before,
                "case {case:?} / {label:?}: expected the stored record untouched ({before}), got {reread}"
            );

            // The policy in force is still in force, so every route table the core network
            // holds answers exactly what it held before the update.
            assert_route_tables(&emulator, case, id).await;
        }
    }
}

#[tokio::test]
async fn an_accepted_change_moves_the_core_network_and_leaves_its_route_tables_readable() {
    let emulator = Emulator::new();
    let rows = description_cases();

    let mut seeded = Vec::new();
    for row in &rows {
        seeded.push(seed(&emulator, row.case, &row.description).await);
    }

    // The create's own transition is past, so the one each row observes is its update's.
    tokio::time::sleep(PAST_SETTLE).await;

    for (row, id) in rows.iter().zip(&seeded) {
        let case = row.case;
        // A value no row is seeded with, so this request does ask for a change whichever
        // row it is made on.
        let reconfigured = format!("reconfigured: {case}");
        let (status, answer) = emulator
            .patch(
                &format!("/core-networks/{id}"),
                json!({ "Description": reconfigured }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer}"
        );
        let core_network = &answer["CoreNetwork"];
        assert_eq!(
            core_network["State"],
            json!(UPDATING),
            "case {case:?}: an accepted change owes the transition out of {UPDATING}, got {core_network}"
        );
        assert_eq!(
            core_network["Description"],
            json!(reconfigured),
            "case {case:?}: expected the Description the change wrote, got {core_network}"
        );

        // A reconfiguration deploys one policy over another, so the policy in force is in
        // force throughout: every route table is still readable while the core network is
        // reconfiguring.
        assert_route_tables(&emulator, case, id).await;
    }

    // Once the transition the clock owes falls due every row is settled again, holding what
    // its change wrote — and nothing was called in between.
    tokio::time::sleep(PAST_SETTLE).await;
    for (row, id) in rows.iter().zip(&seeded) {
        let case = row.case;
        let settled = read_core_network(&emulator, case, id).await;
        assert_eq!(
            settled["State"],
            json!(AVAILABLE),
            "case {case:?}: expected {AVAILABLE} once the transition is due, got {settled}"
        );
        assert_eq!(
            settled["Description"],
            json!(format!("reconfigured: {case}")),
            "case {case:?}: expected the written Description to survive, got {settled}"
        );
        assert_route_tables(&emulator, case, id).await;
    }
}
