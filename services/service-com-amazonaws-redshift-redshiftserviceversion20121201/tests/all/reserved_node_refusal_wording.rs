// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE a reserved-node-family or catalog-metadata refusal carries, end
//! to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The RULES the suite asserts, over tables of inputs rather than one recorded
//! request:
//!
//! * a lookup refusal's MESSAGE belongs to the OPERATION: the fault's status and
//!   code are what the model fixes for every operation, while the sentence around
//!   the identifier differs from operation to operation. The operations recorded
//!   refusals pin to the member-named sentence answer the INPUT MEMBER the
//!   identifier arrived on, the value the request supplied, then ` not found.`;
//!   the reserved-node EXCHANGE family answers its own resource-named sentence.
//!   So the identifier is interpolated and nothing else varies, and the SAME
//!   missing identifier asked about through two operations comes back worded two
//!   ways;
//! * a `MaintenanceTrackName` the catalog does not publish is refused with the
//!   fixed prefix `Invalid cluster track: ` and the requested name, with NO
//!   trailing period;
//! * the exchange-status read refused for naming no filter says that one or BOTH
//!   of the optional members will do, in a sentence with no variable part.
//!
//! The expected sentences are spelled out here rather than read off the crate
//! under test, so a row cannot pass by restating whatever the handler happened to
//! build.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// Which sentence an operation words its not-found refusal with.
///
/// Spelled out in this suite rather than read off the crate under test, so a row
/// cannot pass by restating whatever the handler happened to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wording {
    /// `<Member> <value> not found.` — the input member the value arrived on as
    /// the fixed prefix.
    MemberThenValue(&'static str),
    /// `Reserved Node <value> not found.` — what the exchange family answers
    /// about a `ReservedNodeId`.
    ReservedNode,
    /// `Reserved Node Exchange Request <value> not found.` — what the
    /// exchange-status read answers about a `ReservedNodeExchangeRequestId`.
    ExchangeRequest,
}

/// The sentence `wording` words about `value`.
fn sentence(wording: Wording, value: &str) -> String {
    match wording {
        Wording::MemberThenValue(member) => format!("{member} {value} not found."),
        Wording::ReservedNode => format!("Reserved Node {value} not found."),
        Wording::ExchangeRequest => {
            format!("Reserved Node Exchange Request {value} not found.")
        }
    }
}

/// The sentence a track the catalog does not publish is refused with.
fn invalid_track(name: &str) -> String {
    format!("Invalid cluster track: {name}")
}

/// The whole sentence the exchange-status read answers a request that names
/// neither of its optional filters: one or both will do, and no part of it comes
/// from the request.
const MISSING_FILTER_MESSAGE: &str = "You must supply one or both of the optional ReservedNodeId \
     and ReservedNodeExchangeRequestId parameters.";

/// Identifiers of the FORM this service mints — five lowercase-hex groups —
/// which the exchange operations require before they look anything up.
const WELL_FORMED_UNHELD: [&str; 3] = [
    "00000000-0000-0000-0000-000000000000",
    "11111111-1111-1111-1111-111111111111",
    "6a4bcc87-2002-463c-8d81-0f6c9e4f6014",
];

/// One form parameter, owned so a row can build it from a borrowed value.
fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// POST one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// The form parameters one operation sends to ask about a single identifier:
/// every member it requires, with the identifier in the member being tested.
type Request = fn(&str) -> Vec<(String, String)>;

/// One row of the operation table: the case name, the action, the request it
/// makes about one identifier, the sentence its refusal must carry and the
/// awsQuery code of the fault it is refused with.
type OperationCase = (&'static str, &'static str, Request, Wording, &'static str);

fn offering_only(value: &str) -> Vec<(String, String)> {
    vec![pair("ReservedNodeOfferingId", value)]
}

fn purchase(value: &str) -> Vec<(String, String)> {
    vec![pair("ReservedNodeOfferingId", value), pair("NodeCount", "1")]
}

fn reserved_node_only(value: &str) -> Vec<(String, String)> {
    vec![pair("ReservedNodeId", value)]
}

fn exchange_request_only(value: &str) -> Vec<(String, String)> {
    vec![pair("ReservedNodeExchangeRequestId", value)]
}

/// The operations that dereference an identifier of ANY form: the value never
/// has to look like one this service mints to reach the lookup.
fn any_form_operations() -> Vec<OperationCase> {
    vec![
        (
            "DescribeReservedNodeOfferings",
            "DescribeReservedNodeOfferings",
            offering_only as Request,
            Wording::MemberThenValue("ReservedNodeOfferingId"),
            "ReservedNodeOfferingNotFound",
        ),
        (
            "PurchaseReservedNodeOffering",
            "PurchaseReservedNodeOffering",
            purchase as Request,
            Wording::MemberThenValue("ReservedNodeOfferingId"),
            "ReservedNodeOfferingNotFound",
        ),
        (
            "DescribeReservedNodes",
            "DescribeReservedNodes",
            reserved_node_only as Request,
            Wording::MemberThenValue("ReservedNodeId"),
            "ReservedNodeNotFound",
        ),
    ]
}

/// The exchange operations, which rule a value of another FORM out as a bad
/// parameter before any lookup — so their rows carry only minted-form values.
fn minted_form_operations() -> Vec<OperationCase> {
    vec![
        (
            "GetReservedNodeExchangeOfferings",
            "GetReservedNodeExchangeOfferings",
            reserved_node_only as Request,
            Wording::ReservedNode,
            "ReservedNodeNotFound",
        ),
        (
            "DescribeReservedNodeExchangeStatus on ReservedNodeId",
            "DescribeReservedNodeExchangeStatus",
            reserved_node_only as Request,
            Wording::ReservedNode,
            "ReservedNodeNotFound",
        ),
        (
            "DescribeReservedNodeExchangeStatus on ReservedNodeExchangeRequestId",
            "DescribeReservedNodeExchangeStatus",
            exchange_request_only as Request,
            Wording::ExchangeRequest,
            // The awsQuery code the model fixes for this fault.
            "ReservedNodeExchangeNotFond",
        ),
    ]
}

#[tokio::test]
async fn a_lookup_refusal_carries_the_sentence_its_operation_words() {
    let emulator = Emulator::new();

    // Values spanning what the rule interpolates: a plain name, a name in mixed
    // case, one with surrounding space, a long one, and the minted form this
    // service issues ids in. Every one of them names nothing.
    let long: String = std::iter::repeat_n('o', 120).collect();
    let mut values: Vec<String> = vec![
        "no-such-identifier".to_string(),
        "No-Such-Identifier".to_string(),
        " no-such-identifier ".to_string(),
        long,
    ];
    values.extend(WELL_FORMED_UNHELD.iter().map(|value| (*value).to_string()));

    for value in &values {
        for (case, action, request, wording, code) in any_form_operations() {
            let case = format!("{case} on {value:?}");
            let refused = call(&emulator, action, &request(value)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (404, Some(code.to_string())),
                "case {case}: expected 404 {code}, got {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            let expected = sentence(wording, value);
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {:?}",
                refused.text("Message")
            );
        }
    }

    for value in WELL_FORMED_UNHELD {
        for (case, action, request, wording, code) in minted_form_operations() {
            let case = format!("{case} on {value:?}");
            let refused = call(&emulator, action, &request(value)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (404, Some(code.to_string())),
                "case {case}: expected 404 {code}, got {} / {:?} with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            let expected = sentence(wording, value);
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(expected.as_str()),
                "case {case}: expected the message {expected:?}, got {:?}",
                refused.text("Message")
            );
        }
    }

    // The sentences are DISTINGUISHABLE: one missing `ReservedNodeId`, asked
    // about through the read and through the exchange family, comes back worded
    // two ways rather than collapsed onto one template.
    let shared = WELL_FORMED_UNHELD[2];
    let mut answered: Vec<String> = Vec::new();
    for action in ["DescribeReservedNodes", "GetReservedNodeExchangeOfferings"] {
        let refused = call(&emulator, action, &reserved_node_only(shared)).await;
        answered.push(refused.text("Message").unwrap_or_else(|| {
            panic!(
                "case {action} on {shared:?}: expected a refusal message, got body {}",
                refused.body
            )
        }));
    }
    let mut distinct = answered.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        answered.len(),
        "case the two wordings: expected a different sentence per operation for one identifier, got {answered:?}"
    );
}

#[tokio::test]
async fn an_identifier_that_does_resolve_is_never_refused_as_missing() {
    let emulator = Emulator::new();

    // The other half of the rule: an id the catalog DOES publish, and a
    // reservation this account DOES hold, are served — so the sentences above are
    // the answer to a missing identifier and not to every request.
    let catalog = emulator.call("DescribeReservedNodeOfferings", &[]).await;
    assert_eq!(
        catalog.status, 200,
        "case read the offering catalog: expected 200, got {} with body {}",
        catalog.status, catalog.body
    );
    let published = all_text(&catalog.body, "ReservedNodeOfferingId");
    let offering_id = published
        .first()
        .unwrap_or_else(|| panic!("case read the offering catalog: expected a published offering, got body {}", catalog.body));

    let selected = emulator
        .call(
            "DescribeReservedNodeOfferings",
            &[("ReservedNodeOfferingId", offering_id)],
        )
        .await;
    assert_eq!(
        (selected.status, all_text(&selected.body, "ReservedNodeOfferingId")),
        (200, vec![offering_id.clone()]),
        "case select a published offering: expected just that offering, got {} with body {}",
        selected.status,
        selected.body
    );

    let purchased = emulator
        .call(
            "PurchaseReservedNodeOffering",
            &[("ReservedNodeOfferingId", offering_id), ("NodeCount", "2")],
        )
        .await;
    assert_eq!(
        purchased.status, 200,
        "case buy a published offering: expected 200, got {} with body {}",
        purchased.status, purchased.body
    );
    let reserved_node_id = purchased.text("ReservedNodeId").unwrap_or_else(|| {
        panic!(
            "case buy a published offering: expected the minted reservation id, got body {}",
            purchased.body
        )
    });

    let read_back = emulator
        .call(
            "DescribeReservedNodes",
            &[("ReservedNodeId", reserved_node_id.as_str())],
        )
        .await;
    assert_eq!(
        (read_back.status, all_text(&read_back.body, "ReservedNodeId")),
        (200, vec![reserved_node_id.clone()]),
        "case read the reservation back: expected just that reservation, got {} with body {}",
        read_back.status,
        read_back.body
    );
}

#[tokio::test]
async fn an_unpublished_maintenance_track_is_refused_without_a_trailing_period() {
    let emulator = Emulator::new();

    // Rows differ along the ways a name can fall outside the published catalog:
    // a plain name, a published name in another case, a published name with a
    // character added and one removed, and a name of another shape entirely.
    let published = state::PUBLISHED_MAINTENANCE_TRACKS;
    let upper = published[0].to_ascii_uppercase();
    let extended = format!("{}x", published[0]);
    let truncated = published[0][..published[0].len() - 1].to_string();
    for (case, requested) in [
        ("a name nothing publishes", "no-such-track".to_string()),
        ("a published name in upper case", upper),
        ("a published name with a character added", extended),
        ("a published name missing its last character", truncated),
        ("a name of another shape", "track/2026".to_string()),
    ] {
        let refused = call(
            &emulator,
            "DescribeClusterTracks",
            &[pair("MaintenanceTrackName", &requested)],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, Some("InvalidClusterTrack".to_string())),
            "case {case} ({requested:?}): expected 400 InvalidClusterTrack, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        let expected = invalid_track(&requested);
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case} ({requested:?}): expected the message {expected:?}, got {:?}",
            refused.text("Message")
        );
    }

    // Every track the catalog DOES publish is served, so the refusal above is
    // about the name and not about the operation.
    for track in published {
        let served = call(
            &emulator,
            "DescribeClusterTracks",
            &[pair("MaintenanceTrackName", track)],
        )
        .await;
        // A track's own name is the FIRST `MaintenanceTrackName` in its element;
        // the ones after it name the tracks it can be updated to.
        assert_eq!(
            (served.status, served.text("MaintenanceTrackName")),
            (200, Some(track.to_string())),
            "case the published track {track}: expected just that track, got {} with body {}",
            served.status,
            served.body
        );
    }
}

#[tokio::test]
async fn an_exchange_status_read_with_no_filter_states_the_rule_it_enforces() {
    let emulator = Emulator::new();

    // Rows differ along what else the request carries: nothing, a page size, and
    // a page cursor — none of them says which exchanges to report, and none of
    // them appears in the sentence.
    for (case, params) in [
        ("no parameter at all", vec![]),
        ("only a page size", vec![pair("MaxRecords", "20")]),
        (
            "only an empty page cursor",
            vec![pair("MaxRecords", "20"), pair("Marker", "")],
        ),
    ] {
        let refused = call(&emulator, "DescribeReservedNodeExchangeStatus", &params).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, Some("InvalidParameterCombination".to_string())),
            "case {case}: expected 400 InvalidParameterCombination, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(MISSING_FILTER_MESSAGE),
            "case {case}: expected the message {MISSING_FILTER_MESSAGE:?}, got {:?}",
            refused.text("Message")
        );
    }

    // The rule the sentence states is the one the operation enforces: either
    // member ALONE gets past the combination check, and so do both TOGETHER —
    // each is answered by the lookup instead.
    let unheld = WELL_FORMED_UNHELD[0];
    for (case, params, code) in [
        (
            "only a reservation",
            vec![pair("ReservedNodeId", unheld)],
            "ReservedNodeNotFound",
        ),
        (
            "only an exchange request",
            vec![pair("ReservedNodeExchangeRequestId", unheld)],
            "ReservedNodeExchangeNotFond",
        ),
        (
            "both members together",
            vec![
                pair("ReservedNodeId", unheld),
                pair("ReservedNodeExchangeRequestId", unheld),
            ],
            "ReservedNodeExchangeNotFond",
        ),
    ] {
        let refused = call(&emulator, "DescribeReservedNodeExchangeStatus", &params).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, Some(code.to_string())),
            "case {case}: expected the lookup to answer with 404 {code}, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }
}
