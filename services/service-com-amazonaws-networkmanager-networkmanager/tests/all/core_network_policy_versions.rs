// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of a core network's POLICY VERSION history, driven over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a core network's policy is an APPEND-ONLY history of immutable
//! versions. `CreateCoreNetwork`'s document is version 1 and is the one in force (alias
//! `LIVE`); every `PutCoreNetworkPolicy` appends the next version id and leaves the live
//! one alone; `GetCoreNetworkPolicy` reports the version the request names, or the one its
//! alias selects, from the stored document; and `DeleteCoreNetworkPolicyVersion` removes
//! exactly the version it names and answers it, except the version in force, which it
//! refuses. Nothing here reaches into `crate::state`: a version is only ever read back
//! through an operation.
//!
//! The tables below span that class — histories differing in how many versions they hold,
//! which position in the history the delete targets, how the document differs between
//! versions and whether a version carries a description.
//!
//! The history helpers below (`core_network_with_policy`, `append_versions`, `get_policy`,
//! `policy_uri` and the documents they build) are shared with the suite that pins which
//! version a read addresses (`crate::get_core_network_policy_selection`), so a history is
//! built in one place.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// A policy document naming `segments` in `locations`.
///
/// The document's own key names (`version`, `core-network-configuration`, `asn-ranges`,
/// `edge-locations`, `location`, `segments`, `name`) are the published Cloud WAN core
/// network policy schema's, the same ones `crate::core_network_lifecycle` builds with.
fn policy_document(segments: &[&str], locations: &[&str]) -> String {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": locations
                .iter()
                .map(|location| json!({ "location": location }))
                .collect::<Vec<Value>>(),
        },
        "segments": segments
            .iter()
            .map(|segment| json!({ "name": segment }))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// The document the version `version_id` of a test history carries: one segment naming
/// the version, so a read that answers the wrong version is visible in the document.
pub(crate) fn document_of_version(version_id: i32) -> String {
    policy_document(
        &[&format!("segment-v{version_id}")],
        &["us-west-2", "eu-west-1"],
    )
}

/// The policy of a `GetCoreNetworkPolicy` / `PutCoreNetworkPolicy` /
/// `DeleteCoreNetworkPolicyVersion` response.
pub(crate) fn policy(body: &Value) -> &Value {
    &body["CoreNetworkPolicy"]
}

/// Create a global network and a core network inside it carrying `document`, answering the
/// minted `CoreNetworkId`.
pub(crate) async fn core_network_with_policy(
    emulator: &Emulator,
    case: &str,
    document: &str,
) -> String {
    let global_network_id = emulator.create_global_network(case).await;
    let (status, body) = emulator
        .post(
            "/core-networks",
            json!({
                "GlobalNetworkId": global_network_id,
                "Description": case,
                "PolicyDocument": document,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCoreNetwork expected 200, got {status} with {body}"
    );
    body["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkId in {body}"))
        .to_string()
}

/// `PutCoreNetworkPolicy` with `body`, answering the response.
async fn put_policy(emulator: &Emulator, core_network_id: &str, body: Value) -> (u16, Value) {
    emulator
        .post(
            &format!("/core-networks/{core_network_id}/core-network-policy"),
            body,
        )
        .await
}

/// The URI a read of `core_network_id`'s policy with the query string `query` (empty for
/// none) is addressed to.
///
/// Shared with `crate::get_core_network_policy_selection`, which addresses the same URI for
/// the rows that assert a refusal, so the route template is spelled in one place.
pub(crate) fn policy_uri(core_network_id: &str, query: &str) -> String {
    format!("/core-networks/{core_network_id}/core-network-policy{query}")
}

/// `GetCoreNetworkPolicy` with the query string `query` (empty for none).
pub(crate) async fn get_policy(
    emulator: &Emulator,
    core_network_id: &str,
    query: &str,
) -> (u16, Value) {
    emulator.get(&policy_uri(core_network_id, query)).await
}

/// `DeleteCoreNetworkPolicyVersion` of `version_id`.
async fn delete_version(
    emulator: &Emulator,
    core_network_id: &str,
    version_id: i32,
) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions/{version_id}"
        ))
        .await
}

/// Append `count` versions to `core_network_id`, each with the document and description of
/// its own version id, answering the version ids in the order they were minted.
pub(crate) async fn append_versions(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    count: i32,
) -> Vec<i32> {
    let mut minted = Vec::new();
    for index in 0..count {
        let expected = index + 2;
        let (status, body) = put_policy(
            emulator,
            core_network_id,
            json!({
                "PolicyDocument": document_of_version(expected),
                "Description": format!("version {expected}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutCoreNetworkPolicy expected 200, got {status} with {body}"
        );
        let version_id = policy(&body)["PolicyVersionId"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {body}"));
        minted.push(i32::try_from(version_id).expect("a version id must fit in an i32"));
    }
    minted
}

#[tokio::test]
async fn policy_version_history_round_trips_through_stored_state() {
    let emulator = Emulator::new();
    let live_document = policy_document(&["shared"], &["us-west-2"]);
    let core_network_id = core_network_with_policy(&emulator, "history", &live_document).await;

    // The document a create carries is version 1, and the create executes it: it is the
    // LIVE policy, and its change set is EXECUTING until the span the clock owes falls
    // due.
    let (status, body) = get_policy(&emulator, &core_network_id, "").await;
    assert_eq!(
        status, 200,
        "GetCoreNetworkPolicy expected 200, got {status} with {body}"
    );
    assert_eq!(
        policy(&body)["PolicyVersionId"],
        json!(1),
        "the document a create carries must be version 1, got {body}"
    );
    assert_eq!(
        policy(&body)["Alias"],
        json!("LIVE"),
        "the version a create executes must be the LIVE one, got {body}"
    );
    assert_eq!(
        policy(&body)["PolicyDocument"],
        json!(live_document),
        "the stored document must come back verbatim, got {body}"
    );
    assert_eq!(
        policy(&body)["CoreNetworkId"],
        json!(core_network_id),
        "the policy must name the core network holding it, got {body}"
    );
    assert_eq!(
        policy(&body)["ChangeSetState"],
        json!("EXECUTING"),
        "before the settle span elapses the live version must still be EXECUTING, got {body}"
    );

    // A put appends the NEXT version, which is not in force: it waits to be executed and
    // only the newest version carries the LATEST alias.
    let second_document = policy_document(&["shared", "prod"], &["us-west-2", "eu-west-1"]);
    let (status, put) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": second_document, "Description": "adds prod" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy expected 200, got {status} with {put}"
    );
    assert_eq!(
        policy(&put)["PolicyVersionId"],
        json!(2),
        "a put must append the next version id, got {put}"
    );
    assert_eq!(
        policy(&put)["Alias"],
        json!("LATEST"),
        "a put must leave the LIVE policy where it was, got {put}"
    );
    assert_eq!(
        policy(&put)["ChangeSetState"],
        json!("PENDING_GENERATION"),
        "a freshly put version's change set must still be generating, got {put}"
    );
    assert_eq!(
        policy(&put)["Description"],
        json!("adds prod"),
        "the description must be stored with the version, got {put}"
    );

    // The transitions the clock owes both fall due, and neither needed a caller.
    tokio::time::sleep(PAST_SETTLE).await;
    let (_, live) = get_policy(&emulator, &core_network_id, "?alias=LIVE").await;
    assert_eq!(
        policy(&live)["ChangeSetState"],
        json!("EXECUTION_SUCCEEDED"),
        "once the settle span elapsed the live version's change set must have succeeded, got {live}"
    );
    let (_, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
    assert_eq!(
        policy(&latest)["ChangeSetState"],
        json!("READY_TO_EXECUTE"),
        "once generated, a version waiting to be executed must be READY_TO_EXECUTE, got {latest}"
    );
    assert_eq!(
        policy(&latest)["PolicyVersionId"],
        json!(2),
        "LATEST must select the newest version held, got {latest}"
    );
    assert_eq!(
        policy(&live)["PolicyVersionId"],
        json!(1),
        "LIVE must select the version in force, got {live}"
    );

    // A named version id addresses exactly one version, so it takes precedence over an
    // alias the same request carries.
    let (status, both) = get_policy(
        &emulator,
        &core_network_id,
        "?policyVersionId=1&alias=LATEST",
    )
    .await;
    assert_eq!(
        status, 200,
        "GetCoreNetworkPolicy with both selectors expected 200, got {status} with {both}"
    );
    assert_eq!(
        policy(&both)["PolicyVersionId"],
        json!(1),
        "a named version id must win over an alias, got {both}"
    );

    // A read never changes what the next caller sees.
    let (_, first_read) = get_policy(&emulator, &core_network_id, "?policyVersionId=2").await;
    let (_, second_read) = get_policy(&emulator, &core_network_id, "?policyVersionId=2").await;
    assert_eq!(
        first_read, second_read,
        "GetCoreNetworkPolicy must be a pure read, got {first_read} then {second_read}"
    );
    assert_eq!(
        policy(&first_read)["PolicyDocument"],
        json!(second_document),
        "a version read by id must answer its own document, got {first_read}"
    );

    // The delete answers the version it removed, and the version is gone afterwards.
    let (status, deleted) = delete_version(&emulator, &core_network_id, 2).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetworkPolicyVersion expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        policy(&deleted)["PolicyVersionId"],
        json!(2),
        "the delete must answer the version it removed, got {deleted}"
    );
    assert_eq!(
        policy(&deleted)["PolicyDocument"],
        json!(second_document),
        "the delete must answer the document it removed, got {deleted}"
    );
    let (status, gone) = get_policy(&emulator, &core_network_id, "?policyVersionId=2").await;
    assert_eq!(
        status, 404,
        "a deleted version must be gone, got {status} with {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "a deleted version must read as a missing resource, got {gone}"
    );

    // With the newest version gone, LATEST falls back to the version in force, which
    // carries the LIVE alias because that is the one a caller means by it.
    let (status, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
    assert_eq!(
        status, 200,
        "GetCoreNetworkPolicy(LATEST) expected 200, got {status} with {latest}"
    );
    assert_eq!(
        policy(&latest)["PolicyVersionId"],
        json!(1),
        "LATEST must select the newest version still held, got {latest}"
    );
    assert_eq!(
        policy(&latest)["Alias"],
        json!("LIVE"),
        "the version in force keeps the LIVE alias, got {latest}"
    );

    // A version id is never reused: the history is append-only, so the next put takes 3
    // even though 2 is free again.
    let (status, reput) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(3) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy expected 200, got {status} with {reput}"
    );
    assert_eq!(
        policy(&reput)["PolicyVersionId"],
        json!(3),
        "a deleted version id must not be reused, got {reput}"
    );
    assert_eq!(
        policy(&reput)["Description"],
        Value::Null,
        "a put without a description must store none, got {reput}"
    );

    // The LIVE version cannot be deleted, and the refusal removes nothing.
    let (status, refused) = delete_version(&emulator, &core_network_id, 1).await;
    assert_eq!(
        status, 409,
        "deleting the LIVE version must be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "the LIVE refusal must be a conflict, got {refused}"
    );
    assert_eq!(
        refused["ResourceType"],
        json!("core-network-policy"),
        "the refusal must name the resource type it addresses, got {refused}"
    );
    assert_eq!(
        refused["ResourceId"],
        json!(format!("{core_network_id}/1")),
        "the refusal must name the core network and the version, got {refused}"
    );
    let (status, still_live) = get_policy(&emulator, &core_network_id, "?alias=LIVE").await;
    assert_eq!(
        status, 200,
        "the refused delete must leave the LIVE version readable, got {status} with {still_live}"
    );
    assert_eq!(
        policy(&still_live)["PolicyDocument"],
        json!(live_document),
        "the refused delete must leave the LIVE document untouched, got {still_live}"
    );
}

/// One row of the delete table: a history of `appended` put versions beside the live
/// version 1, and the version the delete names.
struct DeleteCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// How many versions are put on top of the live version 1.
    appended: i32,
    /// The version id the delete names.
    target: i32,
    /// The version ids still held afterwards, in version order.
    surviving: &'static [i32],
    /// The version `LATEST` selects afterwards.
    latest_after: i32,
}
#[tokio::test]
async fn a_delete_removes_exactly_the_version_it_names() {
    let emulator = Emulator::new();
    // The rows differ in how long the history is and where in it the delete lands: the
    // only deletable version, the oldest of several, one in the middle, and the newest.
    let cases = [
        DeleteCase {
            name: "the only version that is not in force",
            appended: 1,
            target: 2,
            surviving: &[1],
            latest_after: 1,
        },
        DeleteCase {
            name: "the oldest of three appended versions",
            appended: 3,
            target: 2,
            surviving: &[1, 3, 4],
            latest_after: 4,
        },
        DeleteCase {
            name: "the middle of three appended versions",
            appended: 3,
            target: 3,
            surviving: &[1, 2, 4],
            latest_after: 4,
        },
        DeleteCase {
            name: "the newest of three appended versions",
            appended: 3,
            target: 4,
            surviving: &[1, 2, 3],
            latest_after: 3,
        },
        DeleteCase {
            name: "the newest of a long history",
            appended: 7,
            target: 8,
            surviving: &[1, 2, 3, 4, 5, 6, 7],
            latest_after: 7,
        },
    ];

    // Each row's core network, kept so the topology check below runs once, after the
    // transition the clock owes them all has fallen due.
    let mut settled: Vec<(&DeleteCase, String)> = Vec::new();

    for case in &cases {
        let live_document = policy_document(&["live"], &["us-west-2"]);
        let core_network_id = core_network_with_policy(&emulator, case.name, &live_document).await;
        let minted = append_versions(&emulator, case.name, &core_network_id, case.appended).await;
        let expected_minted: Vec<i32> = (2..=case.appended + 1).collect();
        assert_eq!(
            minted, expected_minted,
            "case {:?}: a put must append consecutive version ids, got {minted:?}",
            case.name
        );

        let (status, deleted) = delete_version(&emulator, &core_network_id, case.target).await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteCoreNetworkPolicyVersion expected 200, got {status} with {deleted}",
            case.name
        );
        assert_eq!(
            policy(&deleted)["PolicyVersionId"],
            json!(case.target),
            "case {:?}: the delete must answer the version it named, got {deleted}",
            case.name
        );
        assert_eq!(
            policy(&deleted)["PolicyDocument"],
            json!(document_of_version(case.target)),
            "case {:?}: the delete must answer the document it removed, got {deleted}",
            case.name
        );
        assert_eq!(
            policy(&deleted)["Description"],
            json!(format!("version {}", case.target)),
            "case {:?}: the delete must answer the description it removed, got {deleted}",
            case.name
        );

        // Exactly the named version is gone; every other version still answers its own
        // document.
        let (status, gone) = get_policy(
            &emulator,
            &core_network_id,
            &format!("?policyVersionId={}", case.target),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {:?}: the deleted version must be gone, got {status} with {gone}",
            case.name
        );
        for &version_id in case.surviving {
            let (status, held) = get_policy(
                &emulator,
                &core_network_id,
                &format!("?policyVersionId={version_id}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: version {version_id} must survive, got {status} with {held}",
                case.name
            );
            let expected_document = if version_id == 1 {
                live_document.clone()
            } else {
                document_of_version(version_id)
            };
            assert_eq!(
                policy(&held)["PolicyDocument"],
                json!(expected_document),
                "case {:?}: version {version_id} must answer its own document, got {held}",
                case.name
            );
        }

        let (_, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
        assert_eq!(
            policy(&latest)["PolicyVersionId"],
            json!(case.latest_after),
            "case {:?}: LATEST must select the newest version still held, got {latest}",
            case.name
        );
        let (_, live) = get_policy(&emulator, &core_network_id, "?alias=LIVE").await;
        assert_eq!(
            policy(&live)["PolicyVersionId"],
            json!(1),
            "case {:?}: the LIVE version is untouched by a delete, got {live}",
            case.name
        );

        // The core network's reported topology comes from the LIVE document alone, so
        // appending and deleting versions leaves it exactly as the create derived it.
        // Checked below, once the transition the clock owes every one of these core
        // networks has fallen due: a core network reports its topology only once its
        // policy is in force.
        settled.push((case, core_network_id.clone()));
    }

    tokio::time::sleep(PAST_SETTLE).await;
    for (case, core_network_id) in &settled {
        let (status, core_network) = emulator
            .get(&format!("/core-networks/{core_network_id}"))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetCoreNetwork expected 200, got {status} with {core_network}",
            case.name
        );
        assert_eq!(
            core_network["CoreNetwork"]["Segments"],
            json!([{ "Name": "live", "EdgeLocations": ["us-west-2"], "SharedSegments": [] }]),
            "case {:?}: only the LIVE policy shapes the topology, got {core_network}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_policy_version_delete_is_refused_where_the_version_is_not_deletable() {
    let emulator = Emulator::new();
    let live_document = policy_document(&["live"], &["us-west-2"]);
    let core_network_id = core_network_with_policy(&emulator, "refusals", &live_document).await;
    append_versions(&emulator, "refusals", &core_network_id, 1).await;

    // An unknown core network: the 404 names the core network, not a policy.
    let unknown = "core-network-00000000000000bad";
    let (status, body) = delete_version(&emulator, unknown, 1).await;
    assert_eq!(
        status, 404,
        "an unknown core network must be a 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "an unknown core network must read as a missing resource, got {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("core-network"),
        "the 404 must name the resource type that was missing, got {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(unknown),
        "the 404 must name the resource that was missing, got {body}"
    );

    // A version this core network never minted, and one a delete already removed, are
    // both missing resources: the delete is not idempotent.
    for (case, version_id) in [("never minted", 9), ("deleted twice", 2)] {
        if case == "deleted twice" {
            let (status, first) = delete_version(&emulator, &core_network_id, version_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first delete must succeed, got {status} with {first}"
            );
        }
        let (status, body) = delete_version(&emulator, &core_network_id, version_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a missing resource, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("core-network-policy"),
            "case {case:?}: the 404 must name the policy as the missing resource, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(format!("{core_network_id}/{version_id}")),
            "case {case:?}: the 404 must name the core network and the version, got {body}"
        );
    }

    // A core network created without a policy document holds no version at all, so every
    // selector a reader can use answers 404 and so does a delete.
    let bare_global_network = emulator.create_global_network("no policy").await;
    let (status, created) = emulator
        .post(
            "/core-networks",
            json!({ "GlobalNetworkId": bare_global_network }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork without a policy expected 200, got {status} with {created}"
    );
    let bare_id = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();
    // Each row: the selector, and the `<CoreNetworkId>/<selector>` id the 404 names it by.
    for (query, selector) in [
        ("", "LIVE"),
        ("?alias=LIVE", "LIVE"),
        ("?alias=LATEST", "LATEST"),
        ("?policyVersionId=1", "1"),
    ] {
        let (status, body) = get_policy(&emulator, &bare_id, query).await;
        assert_eq!(
            status, 404,
            "case {query:?}: a core network with no policy version must answer 404, got {status} with {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("core-network-policy"),
            "case {query:?}: the 404 must name the policy as the missing resource, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(format!("{bare_id}/{selector}")),
            "case {query:?}: the 404 must name the core network and the selector, got {body}"
        );
    }
    let (status, body) = delete_version(&emulator, &bare_id, 1).await;
    assert_eq!(
        status, 404,
        "a core network with no policy version has nothing to delete, got {status} with {body}"
    );

    // An alias outside the modelled enumeration is a refusal of the member, not a
    // fallback to some other version.
    let (status, body) = get_policy(&emulator, &core_network_id, "?alias=CURRENT").await;
    assert_eq!(
        status, 400,
        "an unmodelled alias must be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "an unmodelled alias must be a validation failure, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("alias"),
        "the refusal must name the member at fault, got {body}"
    );
}

#[tokio::test]
async fn a_put_appends_once_per_request_and_is_guarded_by_its_version_check() {
    let emulator = Emulator::new();
    let live_document = policy_document(&["live"], &["us-west-2"]);
    let core_network_id = core_network_with_policy(&emulator, "puts", &live_document).await;

    // `ClientToken` is the operation's idempotency token: the same request twice appends
    // once.
    let request = json!({
        "PolicyDocument": document_of_version(2),
        "Description": "version 2",
        "ClientToken": "token-one",
    });
    let (status, first) = put_policy(&emulator, &core_network_id, request.clone()).await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy expected 200, got {status} with {first}"
    );
    let (status, retry) = put_policy(&emulator, &core_network_id, request).await;
    assert_eq!(
        status, 200,
        "a retry of the same put expected 200, got {status} with {retry}"
    );
    assert_eq!(
        policy(&retry)["PolicyVersionId"],
        policy(&first)["PolicyVersionId"],
        "a retried put must answer the version it already minted, got {retry}"
    );
    let (status, unminted) = get_policy(&emulator, &core_network_id, "?policyVersionId=3").await;
    assert_eq!(
        status, 404,
        "a retried put must not append a second version, got {status} with {unminted}"
    );

    // The same token carrying a different document is a conflict, and appends nothing.
    let (status, conflict) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(3), "ClientToken": "token-one" }),
    )
    .await;
    assert_eq!(
        status, 409,
        "a token reused for a different request must conflict, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "a reused token must read as a conflict, got {conflict}"
    );

    // `LatestVersionId` is the request's own guard on the version the history ends at: a
    // value that is not the stored latest fails it, appends nothing and is refused as a
    // failure of that input member; the stored latest is accepted. The whole class of
    // failing values is tabled in
    // `a_put_guard_refuses_every_version_id_that_is_not_the_stored_latest`.
    for (case, named, expected_status) in [
        ("stale version", 1, 400),
        ("a version that was never minted", 42, 400),
        ("the current latest version", 2, 200),
    ] {
        let (status, body) = put_policy(
            &emulator,
            &core_network_id,
            json!({
                "PolicyDocument": document_of_version(3),
                "LatestVersionId": named,
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a validation failure, got {body}"
            );
        } else {
            assert_eq!(
                policy(&body)["PolicyVersionId"],
                json!(3),
                "case {case:?}: the accepted put must append the next version, got {body}"
            );
        }
    }

    // A document that does not hold up is refused with the policy exception and appends
    // nothing, so the history still ends where the last accepted put left it.
    let (status, refused) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": "{\"segments\": []}" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a document missing required fields must be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "CoreNetworkPolicyException",
        "a malformed document must read as a policy exception, got {refused}"
    );
    let (_, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
    assert_eq!(
        policy(&latest)["PolicyVersionId"],
        json!(3),
        "a refused put must append nothing, got {latest}"
    );

    // An unknown core network has no history to append to.
    let (status, body) = put_policy(
        &emulator,
        "core-network-00000000000000bad",
        json!({ "PolicyDocument": live_document }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a put against an unknown core network must be a 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a put against an unknown core network must read as a missing resource, got {body}"
    );
}

/// How many policy versions one core network may hold before a put is refused: the cap
/// the append-only history is bounded by, read from the crate under test so the two
/// cannot drift.
const MAX_POLICY_VERSIONS: i32 =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_VERSIONS_PER_CORE_NETWORK
        as i32;

#[tokio::test]
async fn a_create_without_a_policy_stays_idempotent_after_a_put() {
    let emulator = Emulator::new();
    let global_network_id = emulator.create_global_network("no policy, then a put").await;
    // A create that carries no document, with an idempotency token of its own.
    let request = json!({
        "GlobalNetworkId": global_network_id,
        "ClientToken": "create-without-a-policy",
    });
    let (status, created) = emulator.post("/core-networks", request.clone()).await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork without a policy expected 200, got {status} with {created}"
    );
    let core_network_id = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();

    // A put mints version 1 on that core network — a version waiting to be executed, so
    // nothing is in force yet.
    let (status, put) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(1) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy expected 200, got {status} with {put}"
    );
    assert_eq!(
        policy(&put)["PolicyVersionId"],
        json!(1),
        "the first put of a core network created without a document must mint version 1, got {put}"
    );
    assert_eq!(
        policy(&put)["Alias"],
        json!("LATEST"),
        "a put never puts its version in force, got {put}"
    );

    // The create, retried with the same token, is still the same request: what it is
    // idempotent in is the document IT carried (none), not whatever a put has since
    // added.
    let (status, retried) = emulator.post("/core-networks", request).await;
    assert_eq!(
        status, 200,
        "a retried create expected 200, got {status} with {retried}"
    );
    assert_eq!(
        retried["CoreNetwork"]["CoreNetworkId"],
        json!(core_network_id),
        "a retried create must answer the core network it already minted, got {retried}"
    );
}

#[tokio::test]
async fn the_policy_version_history_is_capped_and_a_delete_frees_room_but_never_an_id() {
    let emulator = Emulator::new();
    let live_document = policy_document(&["live"], &["us-west-2"]);
    let core_network_id = core_network_with_policy(&emulator, "capped", &live_document).await;

    // Fill the history to its bound: version 1 came from the create, so that is one put
    // short of the cap.
    let minted = append_versions(
        &emulator,
        "at the bound",
        &core_network_id,
        MAX_POLICY_VERSIONS - 1,
    )
    .await;
    assert_eq!(
        minted.last().copied(),
        Some(MAX_POLICY_VERSIONS),
        "the history must fill to its bound, got {minted:?}"
    );

    // One past the bound is refused, and appends nothing.
    let (status, refused) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(MAX_POLICY_VERSIONS + 1) }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a put past the bound must be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "a put past the bound must read as a validation failure, got {refused}"
    );
    let (_, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
    assert_eq!(
        policy(&latest)["PolicyVersionId"],
        json!(MAX_POLICY_VERSIONS),
        "the refused put must append nothing, got {latest}"
    );

    // Deleting a version frees room for another put — but never its version id, which the
    // next put takes one past the highest ever minted.
    let (status, deleted) = delete_version(&emulator, &core_network_id, 2).await;
    assert_eq!(
        status, 200,
        "deleting a version at the bound expected 200, got {status} with {deleted}"
    );
    let (status, accepted) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(MAX_POLICY_VERSIONS + 1) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a put into the freed room expected 200, got {status} with {accepted}"
    );
    assert_eq!(
        policy(&accepted)["PolicyVersionId"],
        json!(MAX_POLICY_VERSIONS + 1),
        "the freed version id must not be reused, got {accepted}"
    );
}

/// The input member a put's version guard is carried in, as the refusal names it.
const GUARD_MEMBER: &str = "latestVersionId";

/// The prose a failed version guard carries: the service's fixed sentence for the member,
/// naming neither the version asserted nor the one the history ends at, so every failed
/// guard of every core network reads identically.
const GUARD_MESSAGE: &str = "Invalid input. Invalid latest version ID.";

/// Assert that `body` is the refusal a failed version guard answers: HTTP 400 with
/// `ValidationException`, a field-level `Reason` and one `Fields` entry naming
/// [`GUARD_MEMBER`].
fn assert_guard_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: a failed version guard must answer 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: a failed version guard must read as a validation failure, got {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("FieldValidationFailed"),
        "case {case:?}: the refusal must blame the input member, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!(GUARD_MEMBER),
        "case {case:?}: the refusal must name the member at fault, got {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(GUARD_MESSAGE),
        "case {case:?}: expected Message {GUARD_MESSAGE:?}, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Message"].as_str(),
        Some(GUARD_MESSAGE),
        "case {case:?}: the field entry must repeat the envelope's message, got {body}"
    );
}

#[tokio::test]
async fn a_put_guard_refuses_every_version_id_that_is_not_the_stored_latest() {
    let emulator = Emulator::new();
    let live_document = policy_document(&["live"], &["us-west-2"]);
    let core_network_id = core_network_with_policy(&emulator, "guard", &live_document).await;
    // A history of three versions: version 1 from the create, two appended, so the stored
    // latest is 3 and a guard value may sit below it, on it, or above it.
    let minted = append_versions(&emulator, "guard", &core_network_id, 2).await;
    assert_eq!(
        minted,
        vec![2, 3],
        "the history must end at version 3, got {minted:?}"
    );

    // Every value that is not the stored latest fails the guard the same way, whatever
    // side of it it sits on and whether the history ever held it: the rows differ in that,
    // and in magnitude down to the modelled `Integer`'s own bounds.
    for (case, named) in [
        ("the oldest version held", 1),
        ("the version one behind the latest", 2),
        ("the next version id, not minted yet", 4),
        ("a version far past the latest", 42),
        ("zero", 0),
        ("a negative version id", -7),
        ("the largest integer", i32::MAX),
        ("the smallest integer", i32::MIN),
    ] {
        let (status, body) = put_policy(
            &emulator,
            &core_network_id,
            json!({
                "PolicyDocument": document_of_version(4),
                "LatestVersionId": named,
            }),
        )
        .await;
        assert_guard_refusal(case, status, &body);
        // A refused put appends nothing, so the history still ends where it did.
        let (status, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
        assert_eq!(
            (status, &policy(&latest)["PolicyVersionId"]),
            (200, &json!(3)),
            "case {case:?}: a refused put must append nothing, got {status} with {latest}"
        );
    }

    // The stored latest passes the guard, and the version it appended becomes the value the
    // guard now demands: the same number that was accepted a moment ago is refused next.
    let (status, accepted) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(4), "LatestVersionId": 3 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the stored latest must pass the guard, got {status} with {accepted}",
        "the stored latest"
    );
    assert_eq!(
        policy(&accepted)["PolicyVersionId"],
        json!(4),
        "case {:?}: the accepted put must append the next version, got {accepted}",
        "the stored latest"
    );
    let (status, superseded) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(5), "LatestVersionId": 3 }),
    )
    .await;
    assert_guard_refusal("a guard the preceding put superseded", status, &superseded);

    // The guard is an equality against the version the history ENDS at, not against the
    // highest id ever minted: deleting the newest version moves it back, so the id that
    // just passed now fails and the one below it passes.
    let (status, deleted) = delete_version(&emulator, &core_network_id, 4).await;
    assert_eq!(
        status, 200,
        "deleting the newest version expected 200, got {status} with {deleted}"
    );
    let (status, stale) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(5), "LatestVersionId": 4 }),
    )
    .await;
    assert_guard_refusal("the deleted newest version", status, &stale);
    let (status, after_delete) = put_policy(
        &emulator,
        &core_network_id,
        json!({ "PolicyDocument": document_of_version(5), "LatestVersionId": 3 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the version the history ends at must pass the guard, got {status} with {after_delete}",
        "the latest after a delete"
    );
    assert_eq!(
        policy(&after_delete)["PolicyVersionId"],
        json!(5),
        "case {:?}: the accepted put must append the next version, got {after_delete}",
        "the latest after a delete"
    );
    // Round-tripped through the reader: the accepted put stored the document it carried.
    let (status, read_back) = get_policy(&emulator, &core_network_id, "?policyVersionId=5").await;
    assert_eq!(
        (status, &policy(&read_back)["PolicyDocument"]),
        (200, &json!(document_of_version(5))),
        "the accepted put must be readable back as the version it minted, got {status} with {read_back}"
    );

    // A core network that holds no policy version at all has nothing a guard can match, so
    // every value fails it — including the id the first put would mint — while a put
    // carrying no guard appends unconditionally.
    let bare_global_network = emulator.create_global_network("guard, no policy").await;
    let (status, created) = emulator
        .post(
            "/core-networks",
            json!({ "GlobalNetworkId": bare_global_network }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork without a policy expected 200, got {status} with {created}"
    );
    let bare_id = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();
    for (case, named) in [
        ("the id the first put would mint, on an empty history", 1),
        ("zero, on an empty history", 0),
        ("a version far past an empty history", 99),
    ] {
        let (status, body) = put_policy(
            &emulator,
            &bare_id,
            json!({
                "PolicyDocument": document_of_version(1),
                "LatestVersionId": named,
            }),
        )
        .await;
        assert_guard_refusal(case, status, &body);
        let (status, nothing) = get_policy(&emulator, &bare_id, "?alias=LATEST").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused put must leave the history empty, got {status} with {nothing}"
        );
    }
    let (status, first) = put_policy(
        &emulator,
        &bare_id,
        json!({ "PolicyDocument": document_of_version(1) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: a put carrying no guard must append, got {status} with {first}",
        "no guard on an empty history"
    );
    assert_eq!(
        policy(&first)["PolicyVersionId"],
        json!(1),
        "case {:?}: the first put must mint version 1, got {first}",
        "no guard on an empty history"
    );
}
