// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateCoreNetwork`, driven over HTTP through the real router.
//!
//! The rule these suites pin is the one every update of this service implements: an update
//! writes ONLY the members the request carries — an absent member leaves the stored value
//! as it was, a supplied one REPLACES it, the empty string included — and rewrites nothing
//! else. For a core network that means its ids, its ARN, its parent back-reference, its
//! `CreatedAt`, its tags and the topology derived from the policy document it holds all
//! survive the update, while the record itself reads `UPDATING` until the transition the
//! clock owes that reconfiguration falls due and `AVAILABLE` after it. Addressing a core
//! network the store does not hold is the missing-resource answer (404
//! `ResourceNotFoundException`) and stores nothing.
//!
//! Nothing reaches into `crate::state`: a core network is only ever read back through
//! `GetCoreNetwork`. The harness is the one `global_network_lifecycle` defines.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, expected_arn, Emulator, PAST_SETTLE};

/// The `Description` every seeded core network starts with, so a row that supplies no
/// `Description` has a value to leave alone.
const BASE_DESCRIPTION: &str = "baseline description";

/// The text every `ResourceNotFoundException` of this service carries
/// (pinned by the `resource_not_found_message` suite).
const NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The ARN resource type a refusal of this suite reports.
const CORE_NETWORK: &str = "core-network";

/// The `Tags` every seeded core network carries, which no update may rewrite.
fn base_tags() -> Value {
    json!([{ "Key": "env", "Value": "test" }])
}

/// The policy document every seeded core network holds. Its key names
/// (`core-network-configuration`, `asn-ranges`, `edge-locations`, `segments`, `location`,
/// `asn`) are the published Cloud WAN core network policy schema's.
fn base_policy_document() -> Value {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": "us-west-2", "asn": 64512 }],
        },
        "segments": [{ "name": "shared" }],
    })
}

/// The `Edges` the document above configures, once it is in force.
fn base_edges() -> Value {
    json!([{ "EdgeLocation": "us-west-2", "Asn": 64512 }])
}

/// The `Segments` the document above names, once it is in force.
fn base_segments() -> Value {
    json!([{ "Name": "shared", "EdgeLocations": ["us-west-2"], "SharedSegments": [] }])
}

/// One row of the update table: the request body, the `Description` the stored record
/// must hold afterwards (`Value::Null` standing for "the member is absent"), and whether
/// the request ASKS FOR A CHANGE — a request that asks for the state the core network is
/// already in leaves the service nothing to apply.
struct UpdateCase {
    case: &'static str,
    body: Value,
    description: Value,
    changes: bool,
}

/// The rows of the update table. They differ along WHETHER the request carries the one
/// mutable member at all, and along the value's form — the empty string (the documented
/// removal), a single character, the modelled maximum length of `ConstrainedString`
/// (256, `smithy_query query="operation" operation="UpdateCoreNetwork"`), a multi-line and
/// non-ASCII value, and a value identical to the stored one.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a new description replaces the stored one",
            body: json!({ "Description": "updated description" }),
            description: json!("updated description"),
            changes: true,
        },
        UpdateCase {
            case: "no mutable member at all, so nothing changes",
            body: json!({}),
            description: json!(BASE_DESCRIPTION),
            changes: false,
        },
        UpdateCase {
            case: "an empty Description removes the information it held",
            body: json!({ "Description": "" }),
            description: json!(""),
            changes: true,
        },
        UpdateCase {
            case: "a single character",
            body: json!({ "Description": "x" }),
            description: json!("x"),
            changes: true,
        },
        UpdateCase {
            case: "a value at the modelled maximum length of 256 characters",
            body: json!({ "Description": "d".repeat(256) }),
            description: json!("d".repeat(256)),
            changes: true,
        },
        UpdateCase {
            case: "a multi-line and non-ascii value",
            body: json!({ "Description": "line one\nline two \u{1f310}" }),
            description: json!("line one\nline two \u{1f310}"),
            changes: true,
        },
        UpdateCase {
            case: "the value the record already holds",
            body: json!({ "Description": BASE_DESCRIPTION }),
            description: json!(BASE_DESCRIPTION),
            changes: false,
        },
    ]
}

/// Create a global network and, inside it, a core network seeded with
/// [`BASE_DESCRIPTION`], [`base_policy_document`] and [`base_tags`]. Returns the ids of
/// both: a global network holds at most one core network, so every row needs its own
/// parent.
async fn create_seeded_core_network(emulator: &Emulator, label: &str) -> (String, String) {
    let network = emulator.create_global_network(label).await;
    let (status, created) = emulator
        .post(
            "/core-networks",
            json!({
                "GlobalNetworkId": network,
                "Description": BASE_DESCRIPTION,
                "PolicyDocument": base_policy_document().to_string(),
                "Tags": base_tags(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let id = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: no CoreNetworkId in {created}"))
        .to_string();
    (network, id)
}

/// Read one core network back through `GetCoreNetwork`, the implemented reader.
async fn read_core_network(emulator: &Emulator, id: &str) -> Value {
    let (status, body) = emulator.get(&format!("/core-networks/{id}")).await;
    assert_eq!(
        status, 200,
        "GetCoreNetwork({id}): expected 200, got {status} with {body}"
    );
    body["CoreNetwork"].clone()
}

#[tokio::test]
async fn update_core_network_writes_only_the_description_and_round_trips_through_get_core_network() {
    let emulator = Emulator::new();
    let rows = update_cases();

    // One identically seeded core network per row, plus a sibling no row names.
    let mut seeded = Vec::new();
    for index in 0..rows.len() {
        seeded.push(create_seeded_core_network(&emulator, &format!("row-{index}")).await);
    }
    let (_, sibling) = create_seeded_core_network(&emulator, "sibling").await;

    // The create answers the core network already AVAILABLE, so the UPDATING state each
    // row observes below is the one its own update started.
    tokio::time::sleep(PAST_SETTLE).await;

    for (row, (network, id)) in rows.iter().zip(&seeded) {
        let before = read_core_network(&emulator, id).await;
        let created_at = before["CreatedAt"].clone();

        let (status, answer) = emulator
            .patch(&format!("/core-networks/{id}"), row.body.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {answer}",
            row.case
        );
        let core_network = &answer["CoreNetwork"];

        assert_eq!(
            core_network["Description"], row.description,
            "case {}: expected Description {}, got {core_network}",
            row.case, row.description
        );

        // Everything the update does not take as input is the record it already was.
        assert_eq!(
            core_network["CoreNetworkId"],
            json!(id),
            "case {}: CoreNetworkId must be the addressed core network, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["CoreNetworkArn"],
            json!(expected_arn(CORE_NETWORK, id)),
            "case {}: the ARN must not be rewritten, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["GlobalNetworkId"],
            json!(network.clone()),
            "case {}: the parent back-reference must not be rewritten, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["CreatedAt"], created_at,
            "case {}: CreatedAt must stay the create's instant, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["Tags"],
            base_tags(),
            "case {}: Tags must not be rewritten, got {core_network}",
            row.case
        );
        // Applying an accepted change is not instant, so the reconfigured core network
        // reports the transitional state its update is outstanding in — without losing the
        // topology the policy in force configures, asserted below. A request that asks for
        // the state the core network is already in owes no transition, so it is answered in
        // the state it found.
        let expected_state = if row.changes { "UPDATING" } else { "AVAILABLE" };
        assert_eq!(
            core_network["State"],
            json!(expected_state),
            "case {}: expected State {expected_state}, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["Edges"],
            base_edges(),
            "case {}: the derived edges must survive the update, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["Segments"],
            base_segments(),
            "case {}: the derived segments must survive the update, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["NetworkFunctionGroups"],
            json!([]),
            "case {}: the derived network function groups must survive the update, got {core_network}",
            row.case
        );

        // The answer is the STORED record, not a view of the request.
        let stored = read_core_network(&emulator, id).await;
        assert_eq!(
            stored, *core_network,
            "case {}: GetCoreNetwork must answer the record the update returned, got {stored}",
            row.case
        );
        if !row.changes {
            // Nothing was asked for, so nothing was written: the whole record — its state
            // included — is the one the update found.
            assert_eq!(
                stored, before,
                "case {}: a change-free update must write nothing, got {stored}",
                row.case
            );
        }
    }

    // No update reached a core network it did not name.
    let untouched = read_core_network(&emulator, &sibling).await;
    assert_eq!(
        untouched["Description"],
        json!(BASE_DESCRIPTION),
        "case sibling core network: an update of another core network must leave it alone, got {untouched}"
    );
}

#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_record() {
    let emulator = Emulator::new();
    let network = emulator
        .create_global_network("update-core-network-sequence")
        .await;

    // Seeded with NEITHER a description nor a policy document, so a member nothing has
    // supplied stays absent.
    let (status, created) = emulator
        .post("/core-networks", json!({ "GlobalNetworkId": network }))
        .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} with {created}"
    );
    let id = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork: no CoreNetworkId in {created}"))
        .to_string();

    // Each step applies to the record the step before it left behind.
    let steps = vec![
        UpdateCase {
            case: "step 1: an empty update of a record with no description",
            body: json!({}),
            description: Value::Null,
            changes: false,
        },
        UpdateCase {
            case: "step 2: a Description where there was none",
            body: json!({ "Description": "first" }),
            description: json!("first"),
            changes: true,
        },
        UpdateCase {
            case: "step 3: an empty update keeps what step 2 wrote",
            body: json!({}),
            description: json!("first"),
            changes: false,
        },
        UpdateCase {
            case: "step 4: emptying the Description removes it",
            body: json!({ "Description": "" }),
            description: json!(""),
            changes: true,
        },
        UpdateCase {
            case: "step 5: a value over the emptied one",
            body: json!({ "Description": "second" }),
            description: json!("second"),
            changes: true,
        },
    ];

    for step in &steps {
        let (status, answer) = emulator
            .patch(&format!("/core-networks/{id}"), step.body.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} with {answer}",
            step.case
        );
        let stored = read_core_network(&emulator, &id).await;
        assert_eq!(
            stored["Description"], step.description,
            "case {}: expected the stored Description {}, got {stored}",
            step.case, step.description
        );
    }
}

#[tokio::test]
async fn update_core_network_reports_updating_until_the_transition_the_clock_owes_falls_due() {
    let emulator = Emulator::new();
    let (_, id) = create_seeded_core_network(&emulator, "settling").await;

    // The create answers the core network AVAILABLE with its policy in force, so the
    // transitional state below is the update's own.
    let created = read_core_network(&emulator, &id).await;
    assert_eq!(
        created["State"],
        json!("AVAILABLE"),
        "case before the update: a created core network is AVAILABLE, got {created}"
    );

    let (status, answer) = emulator
        .patch(
            &format!("/core-networks/{id}"),
            json!({ "Description": "updated description" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case while updating: expected 200, got {status} with {answer}"
    );
    let core_network = &answer["CoreNetwork"];
    assert_eq!(
        core_network["State"],
        json!("UPDATING"),
        "case while updating: expected the update's state, got {core_network}"
    );
    assert_eq!(
        core_network["Description"],
        json!("updated description"),
        "case while updating: the description must be written, got {core_network}"
    );
    // The policy in force is still in force while the update is outstanding.
    assert_eq!(
        core_network["Edges"],
        base_edges(),
        "case while updating: expected the edges the policy configures, got {core_network}"
    );

    // Once the transition the clock owes falls due the record is AVAILABLE again, with the
    // description the update wrote still there — and nothing called in between.
    tokio::time::sleep(PAST_SETTLE).await;
    let stored = read_core_network(&emulator, &id).await;
    assert_eq!(
        stored["State"],
        json!("AVAILABLE"),
        "case past the settle span: expected AVAILABLE, got {stored}"
    );
    assert_eq!(
        stored["Description"],
        json!("updated description"),
        "case past the settle span: the update must have persisted, got {stored}"
    );
    assert_eq!(
        stored["Edges"],
        base_edges(),
        "case past the settle span: expected the edges the policy configures, got {stored}"
    );
}

/// One refusal row: the id the request addresses, and how it came to name nothing.
struct NotFoundCase {
    case: &'static str,
    core_network: String,
}

#[tokio::test]
async fn update_core_network_refuses_a_core_network_the_store_does_not_hold_and_stores_nothing() {
    let emulator = Emulator::new();
    let (_, live) = create_seeded_core_network(&emulator, "owner").await;

    let (_, gone) = create_seeded_core_network(&emulator, "doomed").await;
    let (status, deleted) = emulator.delete(&format!("/core-networks/{gone}")).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetwork: expected 200, got {status} with {deleted}"
    );

    let rows = vec![
        NotFoundCase {
            case: "a core network that never existed",
            // Well formed under the modelled pattern ^core-network-([0-9a-f]{8,17})$,
            // and an id the store never minted.
            core_network: "core-network-00000000000000099".to_string(),
        },
        NotFoundCase {
            case: "a deleted core network",
            core_network: gone.clone(),
        },
    ];

    for row in &rows {
        let (status, answer) = emulator
            .patch(
                &format!("/core-networks/{}", row.core_network),
                json!({ "Description": "must not be stored" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {}: expected 404, got {status} with {answer}",
            row.case
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {}: wrong error code in {answer}",
            row.case
        );
        assert_eq!(
            answer["Message"].as_str(),
            Some(NOT_FOUND_MESSAGE),
            "case {}: expected the constant message {NOT_FOUND_MESSAGE:?}, got {answer}",
            row.case
        );
        assert_eq!(
            answer["ResourceId"].as_str(),
            Some(row.core_network.as_str()),
            "case {}: expected ResourceId {:?}, got {answer}",
            row.case,
            row.core_network
        );
        assert_eq!(
            answer["ResourceType"].as_str(),
            Some(CORE_NETWORK),
            "case {}: expected ResourceType {CORE_NETWORK:?}, got {answer}",
            row.case
        );
    }

    // Every refusal above stored nothing: the live core network still holds what it was
    // seeded with.
    let stored = read_core_network(&emulator, &live).await;
    assert_eq!(
        stored["Description"],
        json!(BASE_DESCRIPTION),
        "case refused updates: the core network must keep its stored Description, got {stored}"
    );
}
