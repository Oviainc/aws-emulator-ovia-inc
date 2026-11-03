// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListCoreNetworkPolicyVersions`, driven over HTTP through the
//! real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the listing reports ONE summary per policy version the addressed
//! core network still holds — the version id, the alias the core network gives it, the
//! description it was stored with, when it was stored and the state of its change set —
//! in version-id order, narrowed to one page by `MaxResults`/`NextToken`, and it changes
//! nothing. A core network holding no version is an empty list, an unknown core network
//! is `ResourceNotFoundException` (404) and a token this service did not mint is
//! `ValidationException` (400).
//!
//! The tables below span that class — histories differing in how they were built (a
//! document carried by the create or not, puts, a delete inside the history, a delete of
//! the newest version, a restore), in how many versions they hold (none, one, several,
//! past ten so a page token's order cannot be the lexicographic order of an unpadded id),
//! and in the page size the request asks for, including each published bound of
//! `MaxResults` and one past it. Nothing here reaches into `crate::state`: every version
//! is read back through an operation.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The members of `ChangeSetState` (`smithy_query shape=ChangeSetState`): the states a
/// listed version's change set may report.
const CHANGE_SET_STATES: [&str; 6] = [
    "PENDING_GENERATION",
    "FAILED_GENERATION",
    "READY_TO_EXECUTE",
    "EXECUTING",
    "EXECUTION_SUCCEEDED",
    "OUT_OF_DATE",
];

/// The published bounds of `MaxResults` (`smithy_query operation=ListCoreNetworkPolicyVersions`:
/// range 1..500).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 500;

/// One step of a history the table below builds on a core network.
#[derive(Clone, Copy)]
enum Step {
    /// `PutCoreNetworkPolicy` of a document naming `self.0`, appending the next version.
    Put(&'static str),
    /// `DeleteCoreNetworkPolicyVersion` of the version id given.
    Delete(i64),
    /// `RestoreCoreNetworkPolicyVersion` of the version id given.
    Restore(i64),
}

/// A policy document naming one segment, so two documents of a history differ.
///
/// The key names (`version`, `core-network-configuration`, `asn-ranges`,
/// `edge-locations`, `location`, `segments`, `name`) are the published Cloud WAN core
/// network policy schema's, the same ones `crate::core_network_policy_versions` builds
/// with.
fn policy_document(segment: &str) -> String {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": "us-west-2" }],
        },
        "segments": [{ "name": segment }],
    })
    .to_string()
}

/// Create a global network and a core network inside it, carrying a document where the row
/// asks for one, answering the minted `CoreNetworkId`.
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

/// Apply `step` to `core_network_id`, asserting it succeeded.
async fn apply(emulator: &Emulator, case: &str, core_network_id: &str, step: Step) {
    let (operation, (status, body)) = match step {
        Step::Put(segment) => (
            "PutCoreNetworkPolicy",
            emulator
                .post(
                    &format!("/core-networks/{core_network_id}/core-network-policy"),
                    json!({
                        "PolicyDocument": policy_document(segment),
                        "Description": format!("the {segment} policy"),
                    }),
                )
                .await,
        ),
        Step::Delete(version_id) => (
            "DeleteCoreNetworkPolicyVersion",
            emulator
                .delete(&format!(
                    "/core-networks/{core_network_id}/core-network-policy-versions/{version_id}"
                ))
                .await,
        ),
        Step::Restore(version_id) => (
            "RestoreCoreNetworkPolicyVersion",
            emulator
                .post(
                    &format!(
                        "/core-networks/{core_network_id}/core-network-policy-versions/{version_id}/restore"
                    ),
                    json!({}),
                )
                .await,
        ),
    };
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} with {body}"
    );
}

/// Build a history: a core network carrying `document` where the row gives one, then every
/// step of `steps` in order. Answers the minted `CoreNetworkId`.
async fn history(
    emulator: &Emulator,
    case: &str,
    document: Option<&str>,
    steps: &[Step],
) -> String {
    let core_network_id = core_network(emulator, case, document).await;
    for step in steps {
        apply(emulator, case, &core_network_id, *step).await;
    }
    core_network_id
}

/// `ListCoreNetworkPolicyVersions` of `core_network_id` with the query string `query`
/// (empty for none).
async fn list_versions(emulator: &Emulator, core_network_id: &str, query: &str) -> (u16, Value) {
    emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions{query}"
        ))
        .await
}

/// `GetCoreNetworkPolicy` of one version of `core_network_id`.
async fn get_policy(emulator: &Emulator, core_network_id: &str, version_id: i64) -> (u16, Value) {
    emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-policy?policyVersionId={version_id}"
        ))
        .await
}

/// The `CoreNetworkPolicyVersions` of a successful response, in the order returned.
fn versions_of(case: &str, body: &Value) -> Vec<Value> {
    body["CoreNetworkPolicyVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkPolicyVersions array in {body}"))
        .clone()
}

/// Check one summary's structure and answer it rendered as `"<PolicyVersionId>/<Alias>"`,
/// with `-` for a version carrying no alias.
///
/// The structural rule checked here holds for every summary whatever the row: it names the
/// core network the request addressed, carries the instant it was stored and a modelled
/// `ChangeSetState`, and carries no policy document — the summary a listing reports is the
/// version's identity and status, never its document (`smithy_query
/// shape=CoreNetworkPolicyVersion`).
fn rendered(case: &str, core_network_id: &str, version: &Value) -> String {
    let version_id = version["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {version}"));
    assert_eq!(
        version["CoreNetworkId"].as_str(),
        Some(core_network_id),
        "case {case:?}: a summary must name the core network addressed, got {version}"
    );
    assert!(
        version["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {case:?}: expected a CreatedAt, got {version}"
    );
    let change_set_state = version["ChangeSetState"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ChangeSetState in {version}"));
    assert!(
        CHANGE_SET_STATES.contains(&change_set_state),
        "case {case:?}: expected a modelled ChangeSetState, got {change_set_state:?} in {version}"
    );
    assert!(
        version["PolicyDocument"].is_null() && version["PolicyErrors"].is_null(),
        "case {case:?}: a summary carries no policy document, got {version}"
    );
    format!("{version_id}/{}", version["Alias"].as_str().unwrap_or("-"))
}

/// Every version of `core_network_id`, rendered and in the order returned, from a single
/// page holding the whole history.
async fn rendered_versions(emulator: &Emulator, case: &str, core_network_id: &str) -> Vec<String> {
    let (status, body) = list_versions(emulator, core_network_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCoreNetworkPolicyVersions expected 200, got {status} with {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case:?}: a page holding every version carries no NextToken, got {body}"
    );
    versions_of(case, &body)
        .iter()
        .map(|version| rendered(case, core_network_id, version))
        .collect()
}

/// The documents the rows below start from.
fn first_document() -> String {
    policy_document("alpha")
}

/// One row of the table below: a history and the summaries the listing must report for it.
///
/// A `type` of its own because the row is a four-tuple, which clippy refuses inline.
type Row<'a> = (&'a str, Option<&'a str>, Vec<Step>, Vec<&'a str>);

#[tokio::test]
async fn the_listing_reports_every_version_its_core_network_still_holds() {
    let emulator = Emulator::new();
    let first = first_document();
    // (case, the document the create carries, the steps after it, the summaries expected)
    let table: Vec<Row<'_>> = vec![
        (
            "a create's document is version 1 and is the one in force",
            Some(first.as_str()),
            vec![],
            vec!["1/LIVE"],
        ),
        (
            "a core network created without a document holds no version at all",
            None,
            vec![],
            vec![],
        ),
        (
            "a put on a core network with no policy mints version 1, unexecuted",
            None,
            vec![Step::Put("beta")],
            vec!["1/LATEST"],
        ),
        (
            "every put appends the next version and leaves the live one alone",
            Some(first.as_str()),
            vec![Step::Put("beta"), Step::Put("gamma"), Step::Put("delta")],
            vec!["1/LIVE", "2/-", "3/-", "4/LATEST"],
        ),
        (
            "deleting a version inside the history removes exactly it",
            Some(first.as_str()),
            vec![
                Step::Put("beta"),
                Step::Put("gamma"),
                Step::Put("delta"),
                Step::Delete(3),
            ],
            vec!["1/LIVE", "2/-", "4/LATEST"],
        ),
        (
            "deleting the newest version moves LATEST to the newest one remaining",
            Some(first.as_str()),
            vec![Step::Put("beta"), Step::Put("gamma"), Step::Delete(3)],
            vec!["1/LIVE", "2/LATEST"],
        ),
        (
            "a restore appends the restored document as a new version",
            Some(first.as_str()),
            vec![Step::Put("beta"), Step::Restore(2)],
            vec!["1/LIVE", "2/-", "3/LATEST"],
        ),
        (
            "a version id past nine is still reported in numeric order",
            Some(first.as_str()),
            vec![
                Step::Put("v2"),
                Step::Put("v3"),
                Step::Put("v4"),
                Step::Put("v5"),
                Step::Put("v6"),
                Step::Put("v7"),
                Step::Put("v8"),
                Step::Put("v9"),
                Step::Put("v10"),
                Step::Put("v11"),
            ],
            vec![
                "1/LIVE", "2/-", "3/-", "4/-", "5/-", "6/-", "7/-", "8/-", "9/-", "10/-",
                "11/LATEST",
            ],
        ),
    ];

    for (case, document, steps, expected) in table {
        let core_network_id = history(&emulator, case, document, &steps).await;
        let listed = rendered_versions(&emulator, case, &core_network_id).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the versions {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_summary_agrees_with_a_read_of_that_version() {
    let emulator = Emulator::new();
    let case = "a history built by a create, two puts and a restore";
    let first = first_document();
    let core_network_id = history(
        &emulator,
        case,
        Some(first.as_str()),
        &[Step::Put("beta"), Step::Put("gamma"), Step::Restore(2)],
    )
    .await;

    let (status, body) = list_versions(&emulator, &core_network_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCoreNetworkPolicyVersions expected 200, got {status} with {body}"
    );
    let listed = versions_of(case, &body);
    assert_eq!(
        listed.len(),
        4,
        "case {case:?}: expected four versions, got {listed:?}"
    );

    for summary in &listed {
        let version_id = summary["PolicyVersionId"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {summary}"));
        let (status, read) = get_policy(&emulator, &core_network_id, version_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCoreNetworkPolicy({version_id}) expected 200, got {status} with {read}"
        );
        let policy = &read["CoreNetworkPolicy"];
        // Every member the summary carries is the one the read of that version reports:
        // a listing and a read answer from the same stored version, never two copies.
        for member in [
            "CoreNetworkId",
            "PolicyVersionId",
            "Alias",
            "Description",
            "CreatedAt",
            "ChangeSetState",
        ] {
            assert_eq!(
                summary[member], policy[member],
                "case {case:?}: version {version_id}'s {member} must agree, got {summary} and {policy}"
            );
        }
        assert!(
            policy["PolicyDocument"].as_str().is_some(),
            "case {case:?}: GetCoreNetworkPolicy({version_id}) must carry the document, got {read}"
        );
    }

    // The description a put stored travels with its version, so the summaries report the
    // descriptions the history was built with and not one another's.
    let descriptions: Vec<Option<&str>> = listed
        .iter()
        .map(|summary| summary["Description"].as_str())
        .collect();
    assert_eq!(
        descriptions,
        vec![
            // The `Description` of a create describes the CORE NETWORK, not the policy
            // version its document becomes, so version 1 carries none.
            None,
            Some("the beta policy"),
            Some("the gamma policy"),
            // A restore reproduces the submission of the version it restores, description
            // included.
            Some("the beta policy"),
        ],
        "case {case:?}: expected each version's own description, got {descriptions:?}"
    );
}

#[tokio::test]
async fn a_listed_change_set_state_follows_the_clock_and_the_listing_writes_nothing() {
    let emulator = Emulator::new();
    let case = "the version a create carries is being executed while a put waits";
    let first = first_document();
    let core_network_id =
        history(&emulator, case, Some(first.as_str()), &[Step::Put("beta")]).await;

    let states = |body: &Value| -> Vec<String> {
        versions_of(case, body)
            .iter()
            .map(|version| version["ChangeSetState"].as_str().unwrap_or("-").to_string())
            .collect()
    };

    let (_, before) = list_versions(&emulator, &core_network_id, "").await;
    assert_eq!(
        states(&before),
        vec!["EXECUTING".to_string(), "PENDING_GENERATION".to_string()],
        "case {case:?}: expected the executed version EXECUTING and the put pending, got {before}"
    );

    let case = "a read reports the same history every time";
    let (_, again) = list_versions(&emulator, &core_network_id, "").await;
    assert_eq!(
        again, before,
        "case {case:?}: expected two reads before the span elapses to agree, got {again} and {before}"
    );

    tokio::time::sleep(PAST_SETTLE).await;

    let case = "once the span the clock owes elapses both versions have settled";
    let (_, after) = list_versions(&emulator, &core_network_id, "").await;
    assert_eq!(
        states(&after),
        vec![
            "EXECUTION_SUCCEEDED".to_string(),
            "READY_TO_EXECUTE".to_string()
        ],
        "case {case:?}: expected the settled states, got {after}"
    );

    let case = "the listing changed nothing a later caller sees";
    let (_, settled) = list_versions(&emulator, &core_network_id, "").await;
    assert_eq!(
        settled, after,
        "case {case:?}: expected two reads of a settled history to agree, got {settled} and {after}"
    );
    let (status, read) = get_policy(&emulator, &core_network_id, 1).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkPolicy expected 200 after the listing, got {status} with {read}"
    );
    assert_eq!(
        read["CoreNetworkPolicy"]["Alias"],
        json!("LIVE"),
        "case {case:?}: the version in force must still be the live one, got {read}"
    );
}

#[tokio::test]
async fn the_listing_pages_every_version_exactly_once_for_each_page_size() {
    let emulator = Emulator::new();
    let case = "a history of eleven versions";
    let first = first_document();
    let steps: Vec<Step> = vec![
        Step::Put("v2"),
        Step::Put("v3"),
        Step::Put("v4"),
        Step::Put("v5"),
        Step::Put("v6"),
        Step::Put("v7"),
        Step::Put("v8"),
        Step::Put("v9"),
        Step::Put("v10"),
        Step::Put("v11"),
    ];
    let core_network_id = history(&emulator, case, Some(first.as_str()), &steps).await;
    let whole = rendered_versions(&emulator, case, &core_network_id).await;
    assert_eq!(
        whole.len(),
        11,
        "case {case:?}: expected eleven versions to page over, got {whole:?}"
    );

    for page_size in 1..=12usize {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let (status, body) = list_versions(&emulator, &core_network_id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListCoreNetworkPolicyVersions expected 200, got {status} with {body}"
            );
            let page = versions_of(&case, &body);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: expected 1 to {page_size} versions on a page, got {}",
                page.len()
            );
            walked.extend(
                page.iter()
                    .map(|version| rendered(&case, &core_network_id, version)),
            );
            match body["NextToken"].as_str() {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected every version exactly once in order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_core_network_a_bad_token_or_a_page_size_outside_the_bounds_is_refused() {
    let emulator = Emulator::new();
    let case = "a core network holding one executed version";
    let first = first_document();
    let core_network_id = core_network(&emulator, case, Some(first.as_str())).await;

    // (case, the core network addressed, the query, the status and code expected)
    let table: Vec<(&str, String, String, u16, &str)> = vec![
        (
            "an unknown core network is not found",
            "core-network-00000000000000000".to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a token this service did not mint is refused",
            core_network_id.clone(),
            "?nextToken=not-a-token-this-service-minted".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a page size below the published minimum is refused",
            core_network_id.clone(),
            format!("?maxResults={}", MAX_RESULTS_MIN - 1),
            400,
            "ValidationException",
        ),
        (
            "a page size past the published maximum is refused",
            core_network_id.clone(),
            format!("?maxResults={}", MAX_RESULTS_MAX + 1),
            400,
            "ValidationException",
        ),
    ];

    for (case, addressed, query, expected_status, expected_code) in table {
        let (status, body) = list_versions(&emulator, &addressed, &query).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            body["CoreNetworkPolicyVersions"].is_null(),
            "case {case:?}: a refused request carries no page, got {body}"
        );
    }

    // Each published bound of `MaxResults` is itself accepted, and answers the one version
    // this core network holds.
    for page_size in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let case = format!("maxResults={page_size} is inside the bounds");
        let (status, body) =
            list_versions(&emulator, &core_network_id, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let listed: Vec<String> = versions_of(&case, &body)
            .iter()
            .map(|version| rendered(&case, &core_network_id, version))
            .collect();
        assert_eq!(
            listed,
            vec!["1/LIVE".to_string()],
            "case {case:?}: expected the one version held, got {listed:?}"
        );
    }
}
