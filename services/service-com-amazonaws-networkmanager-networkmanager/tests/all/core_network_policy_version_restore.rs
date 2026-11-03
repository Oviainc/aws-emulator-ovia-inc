// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `RestoreCoreNetworkPolicyVersion`, driven over HTTP through the
//! real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: restoring a policy version SUBMITS the document that version holds
//! as a NEW version at the end of the core network's append-only history — the next
//! `PolicyVersionId`, its own `CreatedAt`, the alias `LATEST`, and a change set that is
//! `PENDING_GENERATION` until the span the clock owes falls due and `READY_TO_EXECUTE`
//! after it. A restore prepares that change set and executes nothing, so the version
//! restored from is untouched and the policy in force stays the `LIVE` one. The version the
//! `LIVE` alias already points at is the one version there is nothing to restore, so naming
//! it is refused with HTTP 400 and appends nothing. A version the
//! core network does not hold — never minted, or since deleted — is restored by nobody: the
//! call is refused with HTTP 404 and appends nothing, and so is a restore past the
//! history's cap, with HTTP 400.
//!
//! The tables below span that class: histories differing in how many versions they hold and
//! in the documents and descriptions those versions carry; restore chains differing in
//! length and in which position of the history each step names (the version in force, a
//! middle version, the newest, and a version an earlier restore minted); and version ids
//! differing in sign, in magnitude, in whether they were ever minted and in whether they
//! were deleted.
//!
//! Nothing here reaches into `crate::state`: a version is only ever read back through
//! `GetCoreNetworkPolicy`.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// How many policy versions one core network may hold: the cap
/// `state::MAX_POLICY_VERSIONS_PER_CORE_NETWORK` fixes, which a put and a restore share.
const MAX_POLICY_VERSIONS: i64 = 100;

/// A policy document carrying `segment`.
///
/// The document's own key names (`version`, `core-network-configuration`, `asn-ranges`,
/// `edge-locations`, `location`, `segments`, `name`) are the published Cloud WAN core
/// network policy schema's.
fn policy_document(segment: &str) -> String {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": "us-west-2" }, { "location": "eu-west-1" }],
        },
        "segments": [{ "name": segment }],
    })
    .to_string()
}

/// The document the version `version_id` of a test history carries: one segment naming the
/// version, so a restore that submits the wrong version's document is visible in it.
fn document_of_version(version_id: i64) -> String {
    policy_document(&format!("segment-v{version_id}"))
}

/// The description the version `version_id` of a test history carries.
fn description_of_version(version_id: i64) -> String {
    format!("version {version_id}")
}

/// Create a global network and a core network inside it carrying `document` as its first
/// policy version, answering the minted `CoreNetworkId`.
async fn core_network_with_policy(
    emulator: &Emulator,
    case: &str,
    document: Option<&str>,
) -> String {
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

/// `RestoreCoreNetworkPolicyVersion` of `version_id`: both members are `@httpLabel`s
/// (`smithy_query query="operation" operation="RestoreCoreNetworkPolicyVersion"`).
async fn restore(emulator: &Emulator, core_network_id: &str, version_id: i64) -> (u16, Value) {
    emulator
        .post(
            &format!(
                "/core-networks/{core_network_id}/core-network-policy-versions/{version_id}/restore"
            ),
            json!({}),
        )
        .await
}

/// `GetCoreNetworkPolicy` with the query string `query` (empty for none), answering the
/// status and, on success, the `CoreNetworkPolicy` member.
async fn get_policy(emulator: &Emulator, core_network_id: &str, query: &str) -> (u16, Value) {
    let (status, body) = emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-policy{query}"
        ))
        .await;
    if status == 200 {
        (status, body["CoreNetworkPolicy"].clone())
    } else {
        (status, body)
    }
}

/// `PutCoreNetworkPolicy` appending the document and description of `version_id`, asserting
/// it minted exactly that version.
async fn put_version(emulator: &Emulator, case: &str, core_network_id: &str, version_id: i64) {
    let (status, body) = emulator
        .post(
            &format!("/core-networks/{core_network_id}/core-network-policy"),
            json!({
                "PolicyDocument": document_of_version(version_id),
                "Description": description_of_version(version_id),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutCoreNetworkPolicy expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["CoreNetworkPolicy"]["PolicyVersionId"],
        json!(version_id),
        "case {case:?}: expected the put to mint version {version_id}, got {body}"
    );
}

/// One row of the restore table: how many versions the history holds before the first
/// restore, and which version each successive restore names.
struct RestoreCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// How many versions the history holds to begin with: version 1 is the document the
    /// create carried and the rest are appended by puts.
    held: i64,
    /// Which `PolicyVersionId` each successive restore names, in order.
    restores: &'static [i64],
}

/// The table: one row per shape the history and the restore chain take.
fn restore_cases() -> Vec<RestoreCase> {
    vec![
        RestoreCase {
            name: "a two-version history, restoring the version not in force",
            held: 2,
            restores: &[2],
        },
        RestoreCase {
            name: "a three-version history, restoring a middle version then the newest",
            held: 3,
            restores: &[2, 3],
        },
        RestoreCase {
            name: "restoring the newest version of a longer history, repeatedly",
            held: 4,
            restores: &[4, 4],
        },
        RestoreCase {
            name: "restoring the version an earlier restore minted",
            held: 2,
            restores: &[2, 3, 4],
        },
    ]
}

/// What one version of one history must keep reporting, recorded as it was minted so the
/// assertions after the settle span can prove the version did not change.
struct StoredVersion {
    version_id: i64,
    document: String,
    description: Value,
    created_at: Value,
}

#[tokio::test]
async fn restoring_a_policy_version_appends_its_document_as_the_next_version() {
    let emulator = Emulator::new();
    let cases = restore_cases();
    let mut histories: Vec<(&RestoreCase, String, Vec<StoredVersion>)> = Vec::new();

    for case in &cases {
        let live_document = document_of_version(1);
        let core_network_id =
            core_network_with_policy(&emulator, case.name, Some(&live_document)).await;

        // The version the create carried is version 1, it is the one in force, and its
        // change set is EXECUTING while the core network is still CREATING.
        let (status, live) = get_policy(&emulator, &core_network_id, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: GetCoreNetworkPolicy expected 200, got {status} with {live}",
            case.name
        );
        assert_eq!(
            live["PolicyVersionId"],
            json!(1),
            "case {:?}: the create's document is version 1, got {live}",
            case.name
        );
        assert_eq!(
            live["Alias"],
            json!("LIVE"),
            "case {:?}: the create's version is the policy in force, got {live}",
            case.name
        );
        assert_eq!(
            live["ChangeSetState"],
            json!("EXECUTING"),
            "case {:?}: the create executes its own change set, got {live}",
            case.name
        );
        let mut versions = vec![StoredVersion {
            version_id: 1,
            document: live_document.clone(),
            // A create's `Description` describes the core network, not its policy, so the
            // version it carries has none.
            description: Value::Null,
            created_at: live["CreatedAt"].clone(),
        }];

        // Fill the history to the width this row asks for, each version carrying its own
        // document and description.
        for version_id in 2..=case.held {
            put_version(&emulator, case.name, &core_network_id, version_id).await;
            versions.push(StoredVersion {
                version_id,
                document: document_of_version(version_id),
                description: json!(description_of_version(version_id)),
                created_at: Value::Null,
            });
        }
        for held in versions.iter_mut().skip(1) {
            let (status, read) = get_policy(
                &emulator,
                &core_network_id,
                &format!("?policyVersionId={}", held.version_id),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: reading version {} expected 200, got {status} with {read}",
                case.name, held.version_id
            );
            held.created_at = read["CreatedAt"].clone();
        }

        for source in case.restores {
            let expected_id = i64::try_from(versions.len()).unwrap_or(i64::MAX) + 1;
            let source_index = usize::try_from(*source - 1).expect("a version id is positive");
            let restored_document = versions[source_index].document.clone();
            let restored_description = versions[source_index].description.clone();
            let restored_created_at = versions[source_index].created_at.clone();

            let (status, body) = restore(&emulator, &core_network_id, *source).await;
            assert_eq!(
                status, 200,
                "case {:?}: restoring version {source} expected 200, got {status} with {body}",
                case.name
            );
            let minted = body["CoreNetworkPolicy"].clone();
            assert_eq!(
                minted["CoreNetworkId"],
                json!(core_network_id),
                "case {:?}: the minted version must name its core network, got {minted}",
                case.name
            );
            assert_eq!(
                minted["PolicyVersionId"],
                json!(expected_id),
                "case {:?}: restoring version {source} must mint version {expected_id}, got {minted}",
                case.name
            );
            assert_eq!(
                minted["PolicyDocument"],
                json!(restored_document),
                "case {:?}: the minted version must carry version {source}'s document, got {minted}",
                case.name
            );
            assert_eq!(
                minted["Description"], restored_description,
                "case {:?}: the minted version must carry version {source}'s description, got {minted}",
                case.name
            );
            assert_eq!(
                minted["Alias"],
                json!("LATEST"),
                "case {:?}: the version just submitted is the latest, got {minted}",
                case.name
            );
            assert_eq!(
                minted["ChangeSetState"],
                json!("PENDING_GENERATION"),
                "case {:?}: a submitted version's change set is still generating, got {minted}",
                case.name
            );
            assert!(
                minted["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
                "case {:?}: the minted version carries its own CreatedAt, got {minted}",
                case.name
            );

            // READ BACK: the version is in the store, not just in the response.
            let (status, read) = get_policy(
                &emulator,
                &core_network_id,
                &format!("?policyVersionId={expected_id}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: reading version {expected_id} expected 200, got {status} with {read}",
                case.name
            );
            assert_eq!(
                read["PolicyDocument"], minted["PolicyDocument"],
                "case {:?}: the stored document must be the one minted, got {read}",
                case.name
            );
            assert_eq!(
                read["CreatedAt"], minted["CreatedAt"],
                "case {:?}: the stored CreatedAt must be the one minted, got {read}",
                case.name
            );

            // The policy in force did not move: a restore prepares a change set, it does
            // not execute one.
            let (status, live) = get_policy(&emulator, &core_network_id, "?alias=LIVE").await;
            assert_eq!(
                status, 200,
                "case {:?}: GetCoreNetworkPolicy(LIVE) expected 200, got {status} with {live}",
                case.name
            );
            assert_eq!(
                live["PolicyVersionId"],
                json!(1),
                "case {:?}: the policy in force is still version 1 after restoring {source}, got {live}",
                case.name
            );
            assert_eq!(
                live["PolicyDocument"],
                json!(versions[0].document),
                "case {:?}: the live document must be untouched after restoring {source}, got {live}",
                case.name
            );

            // The version restored FROM is immutable: it still reports what it did.
            let (status, read) = get_policy(
                &emulator,
                &core_network_id,
                &format!("?policyVersionId={source}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: reading version {source} expected 200, got {status} with {read}",
                case.name
            );
            assert_eq!(
                read["PolicyDocument"],
                json!(restored_document),
                "case {:?}: restoring version {source} must not rewrite its document, got {read}",
                case.name
            );
            assert_eq!(
                read["CreatedAt"], restored_created_at,
                "case {:?}: restoring version {source} must not change it, got {read}",
                case.name
            );

            versions.push(StoredVersion {
                version_id: expected_id,
                document: restored_document,
                description: restored_description,
                created_at: minted["CreatedAt"].clone(),
            });
        }

        histories.push((case, core_network_id, versions));
    }

    // One wait for the change-set generation the clock owes every submitted version, and
    // for the execution the create owes version 1.
    tokio::time::sleep(PAST_SETTLE).await;

    for (case, core_network_id, versions) in &histories {
        let latest = versions.len();
        for (index, held) in versions.iter().enumerate() {
            let (status, read) = get_policy(
                &emulator,
                core_network_id,
                &format!("?policyVersionId={}", held.version_id),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: reading version {} expected 200, got {status} with {read}",
                case.name, held.version_id
            );
            // Version 1 is the one the create executed; every other version was only
            // submitted, so its change set is ready to execute once generated.
            let expected_state = if held.version_id == 1 {
                "EXECUTION_SUCCEEDED"
            } else {
                "READY_TO_EXECUTE"
            };
            assert_eq!(
                read["ChangeSetState"],
                json!(expected_state),
                "case {:?}: version {} must report {expected_state} once the clock owes it, got {read}",
                case.name,
                held.version_id
            );
            // LIVE is the policy in force, LATEST the newest version held, and a version
            // that is neither carries no alias at all.
            let expected_alias = if held.version_id == 1 {
                json!("LIVE")
            } else if index + 1 == latest {
                json!("LATEST")
            } else {
                Value::Null
            };
            assert_eq!(
                read["Alias"], expected_alias,
                "case {:?}: version {} of {latest} must report alias {expected_alias}, got {read}",
                case.name, held.version_id
            );
            assert_eq!(
                read["CreatedAt"], held.created_at,
                "case {:?}: version {} must keep the CreatedAt it was minted with, got {read}",
                case.name, held.version_id
            );
            assert_eq!(
                read["PolicyDocument"],
                json!(held.document),
                "case {:?}: version {} must keep its document, got {read}",
                case.name, held.version_id
            );
        }

        // The history is exactly the versions put plus one per restore: the version past
        // the last one was never minted.
        let past_end = i64::try_from(latest).unwrap_or(i64::MAX) + 1;
        let (status, body) = get_policy(
            &emulator,
            core_network_id,
            &format!("?policyVersionId={past_end}"),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {:?}: version {past_end} was never minted, got {status} with {body}",
            case.name
        );
    }
}

#[tokio::test]
async fn restoring_a_version_the_core_network_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let live_document = document_of_version(1);
    let core_network_id =
        core_network_with_policy(&emulator, "holds two versions", Some(&live_document)).await;
    put_version(&emulator, "holds two versions", &core_network_id, 2).await;

    // Version ids differing in sign and magnitude, none of them one this core network
    // holds: it holds 1 and 2.
    for version_id in [0_i64, 3, 4, 17, 4096, -1] {
        let (status, body) = restore(&emulator, &core_network_id, version_id).await;
        assert_eq!(
            status, 404,
            "case version {version_id}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case version {version_id}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("core-network-policy"),
            "case version {version_id}: the refusal names the resource kind, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(format!("{core_network_id}/{version_id}")),
            "case version {version_id}: the refusal names the version asked for, got {body}"
        );
    }

    // A version that WAS minted and has since been deleted is no longer held either, so
    // restoring it is refused the same way.
    let (status, body) = emulator
        .delete(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions/2"
        ))
        .await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetworkPolicyVersion expected 200, got {status} with {body}"
    );
    let (status, body) = restore(&emulator, &core_network_id, 2).await;
    assert_eq!(
        status, 404,
        "case a deleted version: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case a deleted version: expected ResourceNotFoundException, got {body}"
    );

    // None of those refusals appended anything: the newest version is still the one the
    // create carried, because the put's version was deleted.
    let (status, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
    assert_eq!(
        status, 200,
        "GetCoreNetworkPolicy(LATEST) expected 200, got {status} with {latest}"
    );
    assert_eq!(
        latest["PolicyVersionId"],
        json!(1),
        "a refused restore must append nothing, got {latest}"
    );

    // A core network created WITHOUT a policy document holds no version at all, so there is
    // nothing to restore.
    let bare = core_network_with_policy(&emulator, "holds no policy", None).await;
    for version_id in [1_i64, 2] {
        let (status, body) = restore(&emulator, &bare, version_id).await;
        assert_eq!(
            status, 404,
            "case no policy, version {version_id}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(format!("{bare}/{version_id}")),
            "case no policy, version {version_id}: the refusal names the version asked for, got {body}"
        );
    }

    // A well-formed core-network id naming nothing stored is not found as a CORE NETWORK: a
    // version is looked for only inside a core network that exists.
    let unknown = "core-network-0000000000000beef";
    let (status, body) = restore(&emulator, unknown, 1).await;
    assert_eq!(
        status, 404,
        "case unknown core network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("core-network"),
        "case unknown core network: the refusal names the core network, got {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(unknown),
        "case unknown core network: the refusal names the id asked for, got {body}"
    );
}

#[tokio::test]
async fn a_restore_past_the_history_cap_is_refused_and_appends_nothing() {
    let emulator = Emulator::new();
    let case = "a full history";
    let live_document = document_of_version(1);
    let core_network_id = core_network_with_policy(&emulator, case, Some(&live_document)).await;

    // Fill the history to one short of the cap with puts, then let a restore take the last
    // slot: a restore appends like a put, so it is the same history both bound.
    for version_id in 2..MAX_POLICY_VERSIONS {
        put_version(&emulator, case, &core_network_id, version_id).await;
    }
    let (status, body) = restore(&emulator, &core_network_id, 2).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the restore that fills the last slot expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["CoreNetworkPolicy"]["PolicyVersionId"],
        json!(MAX_POLICY_VERSIONS),
        "case {case:?}: expected version {MAX_POLICY_VERSIONS}, got {body}"
    );

    // At the cap every further restore is refused, whichever version it names, and the
    // refusal faults no input member: the members are all valid, it is the resulting COUNT
    // the core network cannot have.
    for source in [1_i64, 2, MAX_POLICY_VERSIONS] {
        let (status, body) = restore(&emulator, &core_network_id, source).await;
        assert_eq!(
            status, 400,
            "case at the cap, restoring {source}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case at the cap, restoring {source}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Reason"],
            json!("Other"),
            "case at the cap, restoring {source}: expected reason Other, got {body}"
        );
        assert!(
            body["Fields"].is_null(),
            "case at the cap, restoring {source}: no input member is at fault, got {body}"
        );
    }

    // The refusals appended nothing: the newest version is still the last one that fit.
    let (status, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkPolicy(LATEST) expected 200, got {status} with {latest}"
    );
    assert_eq!(
        latest["PolicyVersionId"],
        json!(MAX_POLICY_VERSIONS),
        "case {case:?}: a refused restore must append nothing, got {latest}"
    );

    // Deleting a version frees room, so a restore is accepted again — and never reuses the
    // id of the version that left.
    let (status, body) = emulator
        .delete(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions/2"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteCoreNetworkPolicyVersion expected 200, got {status} with {body}"
    );
    let (status, body) = restore(&emulator, &core_network_id, 3).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a restore after a delete expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["CoreNetworkPolicy"]["PolicyVersionId"],
        json!(MAX_POLICY_VERSIONS + 1),
        "case {case:?}: a version id is never reused, got {body}"
    );
}
