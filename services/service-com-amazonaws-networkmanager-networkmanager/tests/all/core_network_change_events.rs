// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetCoreNetworkChangeEvents`, driven over HTTP through the real
//! router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the change events of a policy version are one event per object the
//! version's document and the policy that was in force when that version was stored
//! configure differently — an object only the new document configures is `ADD`ed, one only
//! the policy in force configured is `REMOVE`d, one both configure differently is
//! `MODIFY`ed, one both configure the same way is no change at all — each event naming that
//! object in its `IdentifierPath` and reporting the status the execution of that version's
//! change set is in. Nothing here reaches into `crate::state`: every event is read back
//! through the operation.
//!
//! The tables below span that class — documents differing in which kind of object changed
//! (edge, segment, network function group, the attachment-policies block), in the action
//! the change takes, in how many edge locations an entry expands over, in how many objects
//! change at once, and in whether any policy was in force to compare against.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The three Regions the documents below configure edges in. Any Region code satisfies
/// `ExternalRegionCode` (`smithy_query shape=CoreNetworkEdge member=EdgeLocation`: length
/// 1..63); these are spelled out so a row's expected paths can name them.
const WEST: (&str, i64) = ("us-west-2", 64512);
/// See [`WEST`].
const EUROPE: (&str, i64) = ("eu-west-1", 64513);
/// See [`WEST`].
const SOUTH: (&str, i64) = ("ap-south-1", 64514);

/// A core network policy document configuring `edges`, `segments`, `groups` and
/// `attachment-policies` `rules`.
///
/// The key names are the published Cloud WAN core network policy schema's, the same ones
/// `crate::core_network_policy_versions` builds with. Every edge names its own `asn`, so no
/// edge takes one from `asn-ranges` and changing one entry of a document is never visible
/// as a change to another.
fn policy_document(
    edges: &[(&str, i64)],
    segments: &[(&str, Option<&[&str]>)],
    groups: &[(&str, Option<&[&str]>)],
    rules: &[(i32, &str)],
) -> String {
    let named = |entries: &[(&str, Option<&[&str]>)]| -> Vec<Value> {
        entries
            .iter()
            .map(|(name, locations)| match locations {
                Some(locations) => json!({ "name": name, "edge-locations": locations }),
                None => json!({ "name": name }),
            })
            .collect()
    };
    let mut document = json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": edges
                .iter()
                .map(|(location, asn)| json!({ "location": location, "asn": asn }))
                .collect::<Vec<Value>>(),
        },
        "segments": named(segments),
        "network-function-groups": named(groups),
    });
    if !rules.is_empty() {
        document["attachment-policies"] = json!(rules
            .iter()
            .map(|(rule_number, segment)| json!({
                "rule-number": rule_number,
                "conditions": [{ "type": "any" }],
                "action": { "association-method": "constant", "segment": segment },
            }))
            .collect::<Vec<Value>>());
    }
    document.to_string()
}

/// Create a global network and a core network inside it, carrying `document` where the row
/// gives one, answering the minted `CoreNetworkId`.
async fn core_network(emulator: &Emulator, case: &str, document: Option<&str>) -> String {
    let global_network_id = emulator.create_global_network(case).await;
    let mut request = json!({ "GlobalNetworkId": global_network_id, "Description": case });
    if let Some(document) = document {
        request["PolicyDocument"] = json!(document);
    }
    let (status, body) = emulator.post("/core-networks", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCoreNetwork expected 200, got {status} with {body}"
    );
    body["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkId in {body}"))
        .to_string()
}

/// `PutCoreNetworkPolicy` of `document`, answering the version id it minted.
async fn put_policy(emulator: &Emulator, case: &str, core_network_id: &str, document: &str) -> i64 {
    let (status, body) = emulator
        .post(
            &format!("/core-networks/{core_network_id}/core-network-policy"),
            json!({ "PolicyDocument": document }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutCoreNetworkPolicy expected 200, got {status} with {body}"
    );
    body["CoreNetworkPolicy"]["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {body}"))
}

/// `GetCoreNetworkChangeEvents` for `version_id`, with the query string `query`.
async fn get_change_events(
    emulator: &Emulator,
    core_network_id: &str,
    version_id: i64,
    query: &str,
) -> (u16, Value) {
    emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-change-events/{version_id}{query}"
        ))
        .await
}

/// The `CoreNetworkChangeEvents` of a successful response, in the order returned.
fn events_of(case: &str, body: &Value) -> Vec<Value> {
    body["CoreNetworkChangeEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkChangeEvents array in {body}"))
        .clone()
}

/// Check one event's structure and answer it rendered as `"<Action> <IdentifierPath>"`.
///
/// The structural rule checked here holds for every event whatever the row: its `Type` is
/// the kind of object the path names, and its `Values` describe exactly that object — the
/// edge location the object sits at, and the segment or group name for the kind that has
/// one — so no event can describe one object and name another.
fn rendered(case: &str, event: &Value) -> String {
    let path = event["IdentifierPath"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no IdentifierPath in {event}"));
    let action = event["Action"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Action in {event}"));
    let parts: Vec<&str> = path.split('/').collect();
    assert_eq!(
        event["Type"].as_str(),
        parts.first().copied(),
        "case {case:?}: Type must be the kind the path names, got {event}"
    );
    assert!(
        ["ADD", "MODIFY", "REMOVE"].contains(&action),
        "case {case:?}: expected a modelled ChangeAction, got {action:?} in {event}"
    );
    assert!(
        event["EventTime"].as_f64().is_some_and(|at| at > 0.0),
        "case {case:?}: expected an EventTime, got {event}"
    );
    assert_eq!(
        event["Values"]["EdgeLocation"].as_str(),
        parts.get(1).copied(),
        "case {case:?}: Values.EdgeLocation must be the edge location the path names, got {event}"
    );
    let named = match parts.first().copied() {
        Some("CORE_NETWORK_SEGMENT") => event["Values"]["SegmentName"].as_str(),
        Some("NETWORK_FUNCTION_GROUP") => event["Values"]["NetworkFunctionGroupName"].as_str(),
        _ => None,
    };
    assert_eq!(
        named,
        parts.get(2).copied(),
        "case {case:?}: Values must name the object the path names, got {event}"
    );
    format!("{action} {path}")
}

/// Every change event of `version_id`, rendered and in the order returned.
async fn rendered_events(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> Vec<String> {
    let (status, body) = get_change_events(emulator, core_network_id, version_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkChangeEvents expected 200, got {status} with {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case:?}: a single page holding every event carries no NextToken, got {body}"
    );
    events_of(case, &body)
        .iter()
        .map(|event| rendered(case, event))
        .collect()
}

/// The statuses the events of `version_id` report, deduplicated.
async fn statuses(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> BTreeSet<String> {
    let (status, body) = get_change_events(emulator, core_network_id, version_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkChangeEvents expected 200, got {status} with {body}"
    );
    events_of(case, &body)
        .iter()
        .map(|event| {
            event["Status"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Status in {event}"))
                .to_string()
        })
        .collect()
}

/// A document the rows below start from: one edge in each of two Regions and one segment
/// present in both.
fn two_region_document() -> String {
    policy_document(&[WEST, EUROPE], &[("alpha", None)], &[], &[])
}

#[tokio::test]
async fn change_events_report_every_object_the_two_policies_configure_differently() {
    let emulator = Emulator::new();
    // (case, the document in force, the document submitted after it, the events expected)
    let table: Vec<(&str, String, String, Vec<&str>)> = vec![
        (
            "a segment added is added at every edge location",
            two_region_document(),
            policy_document(
                &[WEST, EUROPE],
                &[("alpha", None), ("beta", None)],
                &[],
                &[],
            ),
            vec![
                "ADD CORE_NETWORK_SEGMENT/eu-west-1/beta",
                "ADD CORE_NETWORK_SEGMENT/us-west-2/beta",
            ],
        ),
        (
            "a segment removed is removed at every edge location",
            policy_document(
                &[WEST, EUROPE],
                &[("alpha", None), ("beta", None)],
                &[],
                &[],
            ),
            two_region_document(),
            vec![
                "REMOVE CORE_NETWORK_SEGMENT/eu-west-1/beta",
                "REMOVE CORE_NETWORK_SEGMENT/us-west-2/beta",
            ],
        ),
        (
            "an edge whose ASN changed is modified, and nothing else changes",
            two_region_document(),
            policy_document(
                &[("us-west-2", 64520), EUROPE],
                &[("alpha", None)],
                &[],
                &[],
            ),
            vec!["MODIFY CORE_NETWORK_EDGE/us-west-2"],
        ),
        (
            "an edge location added carries the segments that restrict themselves to none",
            policy_document(&[WEST], &[("alpha", None)], &[], &[]),
            two_region_document(),
            vec![
                "ADD CORE_NETWORK_EDGE/eu-west-1",
                "ADD CORE_NETWORK_SEGMENT/eu-west-1/alpha",
            ],
        ),
        (
            "an edge location removed takes those segments with it",
            two_region_document(),
            policy_document(&[WEST], &[("alpha", None)], &[], &[]),
            vec![
                "REMOVE CORE_NETWORK_EDGE/eu-west-1",
                "REMOVE CORE_NETWORK_SEGMENT/eu-west-1/alpha",
            ],
        ),
        (
            "a segment restricted to one edge location leaves the others",
            two_region_document(),
            policy_document(
                &[WEST, EUROPE],
                &[("alpha", Some(&["us-west-2"]))],
                &[],
                &[],
            ),
            vec!["REMOVE CORE_NETWORK_SEGMENT/eu-west-1/alpha"],
        ),
        (
            "a network function group added is added at every edge location",
            policy_document(&[WEST], &[("alpha", None)], &[], &[]),
            policy_document(
                &[WEST],
                &[("alpha", None)],
                &[("inspection", None)],
                &[],
            ),
            vec!["ADD NETWORK_FUNCTION_GROUP/us-west-2/inspection"],
        ),
        (
            "an attachment-policies block added is one change of its own",
            policy_document(&[WEST], &[("alpha", None)], &[], &[]),
            policy_document(&[WEST], &[("alpha", None)], &[], &[(100, "alpha")]),
            vec!["ADD ATTACHMENT_POLICIES_CONFIGURATION"],
        ),
        (
            "a rule added to that block modifies it",
            policy_document(&[WEST], &[("alpha", None)], &[], &[(100, "alpha")]),
            policy_document(
                &[WEST],
                &[("alpha", None)],
                &[],
                &[(100, "alpha"), (200, "alpha")],
            ),
            vec!["MODIFY ATTACHMENT_POLICIES_CONFIGURATION"],
        ),
        (
            "dropping that block removes it",
            policy_document(&[WEST], &[("alpha", None)], &[], &[(100, "alpha")]),
            policy_document(&[WEST], &[("alpha", None)], &[], &[]),
            vec!["REMOVE ATTACHMENT_POLICIES_CONFIGURATION"],
        ),
        (
            "a document that configures the same objects changes nothing",
            two_region_document(),
            two_region_document(),
            vec![],
        ),
        (
            "every kind of object can change at once",
            two_region_document(),
            policy_document(
                &[WEST, SOUTH],
                &[("beta", None)],
                &[("inspection", None)],
                &[(100, "beta")],
            ),
            vec![
                "ADD ATTACHMENT_POLICIES_CONFIGURATION",
                "ADD CORE_NETWORK_EDGE/ap-south-1",
                "REMOVE CORE_NETWORK_EDGE/eu-west-1",
                "ADD CORE_NETWORK_SEGMENT/ap-south-1/beta",
                "REMOVE CORE_NETWORK_SEGMENT/eu-west-1/alpha",
                "REMOVE CORE_NETWORK_SEGMENT/us-west-2/alpha",
                "ADD CORE_NETWORK_SEGMENT/us-west-2/beta",
                "ADD NETWORK_FUNCTION_GROUP/ap-south-1/inspection",
                "ADD NETWORK_FUNCTION_GROUP/us-west-2/inspection",
            ],
        ),
    ];

    for (case, in_force, submitted, expected) in table {
        let core_network_id = core_network(&emulator, case, Some(&in_force)).await;
        let version_id = put_policy(&emulator, case, &core_network_id, &submitted).await;
        let events = rendered_events(&emulator, case, &core_network_id, version_id).await;
        assert_eq!(
            events, expected,
            "case {case:?}: expected the change events {expected:?}, got {events:?}"
        );
    }
}

#[tokio::test]
async fn the_first_version_of_a_policy_adds_every_object_its_document_configures() {
    let emulator = Emulator::new();
    // (case, the document the first version carries, how it is stored, the events expected)
    let table: Vec<(&str, String, bool, Vec<&str>)> = vec![
        (
            "the document a create carries is version 1 and is executed by the create",
            two_region_document(),
            true,
            vec![
                "ADD CORE_NETWORK_EDGE/eu-west-1",
                "ADD CORE_NETWORK_EDGE/us-west-2",
                "ADD CORE_NETWORK_SEGMENT/eu-west-1/alpha",
                "ADD CORE_NETWORK_SEGMENT/us-west-2/alpha",
            ],
        ),
        (
            "a put on a core network created without a document compares against no policy",
            policy_document(
                &[SOUTH],
                &[("alpha", None)],
                &[("inspection", None)],
                &[(100, "alpha")],
            ),
            false,
            vec![
                "ADD ATTACHMENT_POLICIES_CONFIGURATION",
                "ADD CORE_NETWORK_EDGE/ap-south-1",
                "ADD CORE_NETWORK_SEGMENT/ap-south-1/alpha",
                "ADD NETWORK_FUNCTION_GROUP/ap-south-1/inspection",
            ],
        ),
    ];

    for (case, document, by_create, expected) in table {
        let core_network_id = if by_create {
            core_network(&emulator, case, Some(&document)).await
        } else {
            let core_network_id = core_network(&emulator, case, None).await;
            put_policy(&emulator, case, &core_network_id, &document).await;
            core_network_id
        };
        let events = rendered_events(&emulator, case, &core_network_id, 1).await;
        assert_eq!(
            events, expected,
            "case {case:?}: expected the change events {expected:?}, got {events:?}"
        );
    }
}

#[tokio::test]
async fn a_change_event_reports_the_status_of_the_execution_of_its_change_set() {
    let emulator = Emulator::new();
    let case = "the version a create carries is being executed, so its changes are under way";
    let core_network_id = core_network(&emulator, case, Some(&two_region_document())).await;

    let executing = statuses(&emulator, case, &core_network_id, 1).await;
    assert_eq!(
        executing,
        BTreeSet::from(["IN_PROGRESS".to_string()]),
        "case {case:?}: expected every change IN_PROGRESS while the execution is under way, got {executing:?}"
    );
    let (_, before) = get_change_events(&emulator, &core_network_id, 1, "").await;

    let case = "a put only prepares a change set, so its changes have not started";
    let pending = put_policy(
        &emulator,
        case,
        &core_network_id,
        &policy_document(
            &[WEST, EUROPE],
            &[("alpha", None), ("beta", None)],
            &[],
            &[],
        ),
    )
    .await;
    let not_started = statuses(&emulator, case, &core_network_id, pending).await;
    assert_eq!(
        not_started,
        BTreeSet::from(["NOT_STARTED".to_string()]),
        "case {case:?}: expected every change NOT_STARTED for an unexecuted change set, got {not_started:?}"
    );

    tokio::time::sleep(PAST_SETTLE).await;

    let case = "once the execution has succeeded its changes are complete";
    let complete = statuses(&emulator, case, &core_network_id, 1).await;
    assert_eq!(
        complete,
        BTreeSet::from(["COMPLETE".to_string()]),
        "case {case:?}: expected every change COMPLETE once the execution succeeded, got {complete:?}"
    );

    let case = "an unexecuted change set stays not started however long it waits";
    let still_waiting = statuses(&emulator, case, &core_network_id, pending).await;
    assert_eq!(
        still_waiting,
        BTreeSet::from(["NOT_STARTED".to_string()]),
        "case {case:?}: expected every change still NOT_STARTED, got {still_waiting:?}"
    );

    let case = "the status change moved the event time forward";
    let (_, after) = get_change_events(&emulator, &core_network_id, 1, "").await;
    for (executing, complete) in events_of(case, &before)
        .iter()
        .zip(events_of(case, &after).iter())
    {
        let (started, settled) = (
            executing["EventTime"].as_f64().unwrap_or_default(),
            complete["EventTime"].as_f64().unwrap_or_default(),
        );
        assert!(
            settled > started,
            "case {case:?}: expected the completed event later than the in-progress one, got {settled} and {started} in {complete}"
        );
    }

    let case = "a read reports the same events every time";
    let (_, again) = get_change_events(&emulator, &core_network_id, 1, "").await;
    assert_eq!(
        again, after,
        "case {case:?}: expected two reads of a settled execution to agree, got {again} and {after}"
    );

    let case = "the changes of a stored version are not rewritten by a later put";
    let unchanged = rendered_events(&emulator, case, &core_network_id, 1).await;
    assert_eq!(
        unchanged,
        vec![
            "ADD CORE_NETWORK_EDGE/eu-west-1",
            "ADD CORE_NETWORK_EDGE/us-west-2",
            "ADD CORE_NETWORK_SEGMENT/eu-west-1/alpha",
            "ADD CORE_NETWORK_SEGMENT/us-west-2/alpha",
        ],
        "case {case:?}: expected version 1's changes to stand, got {unchanged:?}"
    );
}

#[tokio::test]
async fn change_events_page_every_event_exactly_once_for_each_page_size() {
    let emulator = Emulator::new();
    let case = "a change set carrying nine changes";
    let core_network_id = core_network(&emulator, case, Some(&two_region_document())).await;
    let version_id = put_policy(
        &emulator,
        case,
        &core_network_id,
        &policy_document(
            &[WEST, SOUTH],
            &[("beta", None)],
            &[("inspection", None)],
            &[(100, "beta")],
        ),
    )
    .await;
    let whole = rendered_events(&emulator, case, &core_network_id, version_id).await;
    assert_eq!(
        whole.len(),
        9,
        "case {case:?}: expected nine changes to page over, got {whole:?}"
    );

    for page_size in 1..=10 {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let (status, body) =
                get_change_events(&emulator, &core_network_id, version_id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetCoreNetworkChangeEvents expected 200, got {status} with {body}"
            );
            let page = events_of(&case, &body);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: expected 1 to {page_size} events on a page, got {}",
                page.len()
            );
            walked.extend(page.iter().map(|event| rendered(&case, event)));
            match body["NextToken"].as_str() {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected every event exactly once in order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_core_network_version_or_token_is_refused() {
    let emulator = Emulator::new();
    let case = "a core network holding one executed version";
    let core_network_id = core_network(&emulator, case, Some(&two_region_document())).await;

    // (case, the core network addressed, the version addressed, the query, status, code)
    let table: Vec<(&str, String, i64, String, u16, &str)> = vec![
        (
            "an unknown core network is not found",
            "core-network-00000000000000000".to_string(),
            1,
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a version this core network does not hold is not found",
            core_network_id.clone(),
            7,
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a version id below the first one is not found either",
            core_network_id.clone(),
            0,
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a token this service did not mint is refused",
            core_network_id.clone(),
            1,
            "?nextToken=not-a-token-this-service-minted".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, addressed, version_id, query, expected_status, expected_code) in table {
        let (status, body) = get_change_events(&emulator, &addressed, version_id, &query).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}
