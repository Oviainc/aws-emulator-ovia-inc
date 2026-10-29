// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of WHERE a core network policy document is expanded into the
//! topology this service reports, driven over HTTP through the real router (the harness
//! lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a policy document is expanded into its topology exactly ONCE —
//! where the request carrying it is accepted — and that one expansion is kept beside the
//! version the document was stored as, so the topology a core network reports and the
//! change set a version carries are functions of the DOCUMENT alone. The same document
//! therefore reports the same topology whichever path put it in force: a create that
//! carried it, a put whose change set was executed, or a restore of a stored version that
//! re-submitted it, however long the history it travelled through. Nothing re-derives a
//! topology from stored text, which is what keeps that expansion — the one part of these
//! requests whose size the caller chooses — off every path that holds the store lock, and a
//! read of a core network derives nothing at all: two reads answer the same topology.
//!
//! The table below spans that class: documents differing in how many edge locations they
//! configure, whether an edge names its own ASN or takes one from `asn-ranges`, whether it
//! carries inside CIDR blocks, whether a named entry restricts itself to some edge
//! locations, whether there are network function groups, attachment policies and segment
//! actions, and one document sitting at this service's own list caps, whose size the CALLER
//! chose — the row the rule exists for. Nothing here reaches into `crate::state`: every
//! topology is read back through an operation.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The list caps a policy document of this service is bounded by, taken from the service
/// itself so the "at the caps" row below stays at them if a cap ever moves.
const MAX_EDGE_LOCATIONS: usize =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_EDGE_LOCATIONS;
/// See [`MAX_EDGE_LOCATIONS`].
const MAX_SEGMENTS: usize =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_SEGMENTS;
/// See [`MAX_EDGE_LOCATIONS`].
const MAX_ATTACHMENT_POLICIES: usize =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_ATTACHMENT_POLICIES;
/// See [`MAX_EDGE_LOCATIONS`].
const MAX_CONDITIONS_PER_RULE: usize =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_CONDITIONS_PER_RULE;
/// See [`MAX_EDGE_LOCATIONS`]: how long one edge-location string may be, which is what makes
/// the refusal row one character past it an over-long location.
const MAX_LOCATION_CHARS: usize =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_LOCATION_CHARS;

/// How long one `conditions[].value` of the large row is: nothing caps it, so this is what
/// makes that document hundreds of kilobytes of caller-chosen JSON rather than a few.
const CONDITION_VALUE_CHARS: usize = 256;

/// One `core-network-configuration.edge-locations[]` entry of a document this suite builds:
/// the location, the ASN the entry names for itself (none to take one from `asn-ranges`),
/// and its inside CIDR blocks.
type EdgeEntry = (String, Option<i64>, Option<Vec<String>>);

/// One named `segments[]` or `network-function-groups[]` entry: its name, and the edge
/// locations it restricts itself to (none to be present in every one).
type NamedEntry = (String, Option<Vec<String>>);

/// A policy document carrying `edges`, `segments`, `groups` and the extra top-level blocks
/// `extra` adds.
///
/// The document's own key names (`version`, `core-network-configuration`, `asn-ranges`,
/// `edge-locations`, `location`, `asn`, `inside-cidr-blocks`, `segments`, `name`,
/// `network-function-groups`, `attachment-policies`, `segment-actions`) are the published
/// Cloud WAN core network policy schema's, the same ones `crate::core_network_change_set`
/// builds its documents with.
fn document(
    edges: &[EdgeEntry],
    segments: &[NamedEntry],
    groups: &[NamedEntry],
    extra: Vec<(&str, Value)>,
) -> Value {
    let mut built = json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64999"],
            "edge-locations": edges
                .iter()
                .map(|(location, asn, blocks)| {
                    let mut entry = json!({ "location": location });
                    if let Some(asn) = asn {
                        entry["asn"] = json!(asn);
                    }
                    if let Some(blocks) = blocks {
                        entry["inside-cidr-blocks"] = json!(blocks);
                    }
                    entry
                })
                .collect::<Vec<Value>>(),
        },
        "segments": named_entries(segments),
    });
    if !groups.is_empty() {
        built["network-function-groups"] = Value::Array(named_entries(groups));
    }
    for (key, value) in extra {
        built[key] = value;
    }
    built
}

/// The `segments` / `network-function-groups` entries for `entries`: each names itself and,
/// where it restricts itself to edge locations, lists them.
fn named_entries(entries: &[NamedEntry]) -> Vec<Value> {
    entries
        .iter()
        .map(|(name, locations)| match locations {
            Some(locations) => json!({ "name": name, "edge-locations": locations }),
            None => json!({ "name": name }),
        })
        .collect()
}

/// An edge-location entry naming its own ASN.
fn edge(location: &str, asn: i64) -> EdgeEntry {
    (location.to_string(), Some(asn), None)
}

/// A named entry restricted to no edge location, so it is present in every one.
fn everywhere(name: &str) -> NamedEntry {
    (name.to_string(), None)
}

/// A named entry restricted to the edge locations `locations`.
fn at(name: &str, locations: &[&str]) -> NamedEntry {
    (
        name.to_string(),
        Some(locations.iter().map(|held| (*held).to_string()).collect()),
    )
}

/// The document every history in this suite starts from: the one a create carries, whose
/// topology each row's own topology has to differ from, so putting a row's document in force
/// and putting this one back are both real changes.
fn baseline_document() -> Value {
    document(
        &[edge("ap-northeast-1", 64599)],
        &[everywhere("baseline")],
        &[],
        Vec::new(),
    )
}

/// The document at this service's list caps: every edge location it allows, every segment
/// and every network function group restricted to one of those locations, and every
/// attachment-policies rule carrying the full count of conditions with long values. Several
/// hundred kilobytes of JSON the CALLER chose the size of.
fn document_at_the_caps() -> Value {
    let edges: Vec<EdgeEntry> = (0..MAX_EDGE_LOCATIONS)
        .map(|index| {
            (
                format!("edge-location-{index:03}"),
                // Half the edges name their own ASN; the rest take one from `asn-ranges`.
                (index % 2 == 0).then_some(64512 + index as i64),
                Some(vec![format!("10.{index}.0.0/16")]),
            )
        })
        .collect();
    let segments: Vec<NamedEntry> = (0..MAX_SEGMENTS)
        .map(|index| {
            (
                format!("segment-{index:03}"),
                Some(vec![format!(
                    "edge-location-{:03}",
                    index % MAX_EDGE_LOCATIONS
                )]),
            )
        })
        .collect();
    let groups: Vec<NamedEntry> = (0..MAX_SEGMENTS)
        .map(|index| {
            (
                format!("group-{index:03}"),
                Some(vec![format!(
                    "edge-location-{:03}",
                    index % MAX_EDGE_LOCATIONS
                )]),
            )
        })
        .collect();
    let rules: Vec<Value> = (0..MAX_ATTACHMENT_POLICIES)
        .map(|index| {
            json!({
                "rule-number": index + 1,
                "conditions": (0..MAX_CONDITIONS_PER_RULE)
                    .map(|position| json!({
                        "type": "tag-value",
                        "operator": "equals",
                        "key": format!("key-{index:03}-{position:02}"),
                        "value": "v".repeat(CONDITION_VALUE_CHARS),
                    }))
                    .collect::<Vec<Value>>(),
                "action": { "association-method": "constant", "segment": format!("segment-{index:03}") },
            })
        })
        .collect();
    document(
        &edges,
        &segments,
        &groups,
        vec![("attachment-policies", Value::Array(rules))],
    )
}

/// One row of the table: a policy document, under the name every assertion reports it by.
struct DocumentCase {
    name: &'static str,
    document: Value,
}

/// The table: one row per form a policy document takes, the last one at the caps.
fn document_cases() -> Vec<DocumentCase> {
    vec![
        DocumentCase {
            name: "one edge naming its own ASN, one unrestricted segment",
            document: document(
                &[edge("us-west-2", 64512)],
                &[everywhere("shared")],
                &[],
                Vec::new(),
            ),
        },
        DocumentCase {
            name: "three edges taking ASNs from the range, a restricted and an unrestricted segment",
            document: document(
                &[
                    ("us-east-1".to_string(), None, None),
                    ("eu-west-1".to_string(), None, None),
                    ("eu-west-2".to_string(), None, None),
                ],
                &[at("prod", &["us-east-1", "eu-west-1"]), everywhere("dev")],
                &[],
                Vec::new(),
            ),
        },
        DocumentCase {
            name: "edges carrying inside CIDR blocks, a network function group and attachment policies",
            document: document(
                &[
                    (
                        "us-west-2".to_string(),
                        Some(64520),
                        Some(vec!["10.0.0.0/16".to_string(), "10.1.0.0/16".to_string()]),
                    ),
                    ("ca-central-1".to_string(), Some(64521), None),
                ],
                &[everywhere("shared")],
                &[at("inspection", &["us-west-2"])],
                vec![(
                    "attachment-policies",
                    json!([{
                        "rule-number": 100,
                        "conditions": [{ "type": "attachment-type", "operator": "equals", "value": "vpc" }],
                        "action": { "association-method": "constant", "segment": "shared" },
                    }]),
                )],
            ),
        },
        DocumentCase {
            name: "segment actions sharing routes, sending via a group and creating routes",
            document: document(
                &[edge("us-west-2", 64530), edge("eu-central-1", 64531)],
                &[everywhere("shared"), everywhere("prod")],
                &[everywhere("inspection")],
                vec![(
                    "segment-actions",
                    json!([
                        { "action": "share", "segment": "shared", "share-with": ["prod"] },
                        {
                            "action": "send-via",
                            "segment": "prod",
                            "via": { "network-function-groups": ["inspection"] },
                        },
                        {
                            "action": "create-route",
                            "segment": "shared",
                            "destination-cidr-blocks": ["192.168.0.0/24", "192.168.1.0/24"],
                            "destinations": ["blackhole"],
                        },
                    ]),
                )],
            ),
        },
        DocumentCase {
            name: "a document configuring one edge and no named entry at all",
            document: document(&[edge("sa-east-1", 64540)], &[], &[], Vec::new()),
        },
        DocumentCase {
            name: "a document at this service's list caps, hundreds of kilobytes of it",
            document: document_at_the_caps(),
        },
    ]
}

/// The derived topology one reported core network carries: the three members the policy
/// document in force decides.
fn topology_of(core_network: &Value) -> Value {
    json!([
        core_network["Edges"],
        core_network["Segments"],
        core_network["NetworkFunctionGroups"],
    ])
}

/// Create a global network and a core network inside it carrying `document`, answering the
/// minted `CoreNetworkId`.
async fn core_network_with(emulator: &Emulator, case: &str, document: &Value) -> String {
    let global_network_id = emulator.create_global_network(case).await;
    let (status, body) = emulator
        .post(
            "/core-networks",
            json!({
                "GlobalNetworkId": global_network_id,
                "Description": case,
                "PolicyDocument": document.to_string(),
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

/// The topology the core network `core_network_id` reports, read back through
/// `GetCoreNetwork`.
async fn reported_topology(emulator: &Emulator, case: &str, core_network_id: &str) -> Value {
    let (status, body) = emulator
        .get(&format!("/core-networks/{core_network_id}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetwork expected 200, got {status} with {body}"
    );
    topology_of(&body["CoreNetwork"])
}

/// `PutCoreNetworkPolicy` of `document`, answering the version id it minted.
async fn put_policy(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    document: &Value,
) -> i64 {
    let (status, body) = emulator
        .post(
            &format!("/core-networks/{core_network_id}/core-network-policy"),
            json!({ "PolicyDocument": document.to_string() }),
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

/// `RestoreCoreNetworkPolicyVersion` of `version_id`, answering the version it minted and
/// the document that version carries.
async fn restore_policy(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> (i64, Value) {
    let (status, body) = emulator
        .post(
            &format!(
                "/core-networks/{core_network_id}/core-network-policy-versions/{version_id}/restore"
            ),
            json!({}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RestoreCoreNetworkPolicyVersion({version_id}) expected 200, got {status}"
    );
    let minted = body["CoreNetworkPolicy"]["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in the restore answer"));
    (minted, body["CoreNetworkPolicy"]["PolicyDocument"].clone())
}

/// `ExecuteCoreNetworkChangeSet` of `version_id`'s change set.
async fn execute(emulator: &Emulator, case: &str, core_network_id: &str, version_id: i64) {
    let (status, body) = emulator
        .post(
            &format!(
                "/core-networks/{core_network_id}/core-network-change-sets/{version_id}/execute"
            ),
            json!({}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: executing version {version_id} expected 200, got {status} with {body}"
    );
}

/// The whole change set of one version, read back through `GetCoreNetworkChangeSet`: no
/// `MaxResults` is asked for, so the answer is every change in one page.
async fn change_set(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> Value {
    let (status, body) = emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-change-sets/{version_id}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkChangeSet({version_id}) expected 200, got {status}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case:?}: a complete change set carries no NextToken, got a token for version {version_id}"
    );
    body["CoreNetworkChanges"].clone()
}

/// One row of the refusal table: a document this service does not accept, and the
/// `CoreNetworkPolicyError$ErrorCode` the refusal reports for it
/// (`state::POLICY_ERROR_INVALID_JSON` and its siblings).
struct RefusedDocument {
    name: &'static str,
    document: String,
    error_code: &'static str,
}

/// The table: one row per way a document fails to be one, spanning the refusal codes.
fn refused_documents() -> Vec<RefusedDocument> {
    vec![
        RefusedDocument {
            name: "not JSON at all",
            document: "{not json".to_string(),
            error_code: "INVALID_JSON",
        },
        RefusedDocument {
            name: "JSON that is not an object",
            document: json!(["a list"]).to_string(),
            error_code: "INVALID_TYPE",
        },
        RefusedDocument {
            name: "an object carrying neither required block",
            document: json!({}).to_string(),
            error_code: "MISSING_REQUIRED_FIELD",
        },
        RefusedDocument {
            name: "more segments than the cap allows",
            document: document(
                &[edge("us-west-2", 64512)],
                &(0..=MAX_SEGMENTS)
                    .map(|index| everywhere(&format!("segment-{index:03}")))
                    .collect::<Vec<_>>(),
                &[],
                Vec::new(),
            )
            .to_string(),
            error_code: "TOO_MANY_ENTRIES",
        },
        RefusedDocument {
            name: "an edge location longer than the shape that reports it allows",
            document: document(
                &[edge(&"e".repeat(MAX_LOCATION_CHARS + 1), 64512)],
                &[everywhere("shared")],
                &[],
                Vec::new(),
            )
            .to_string(),
            error_code: "INVALID_VALUE",
        },
    ]
}

#[tokio::test]
async fn a_document_is_judged_before_the_store_is_consulted() {
    let emulator = Emulator::new();

    // The caller's document is the one part of these requests whose size the caller
    // chooses, so it is judged and expanded before this service reaches its store: a
    // request carrying a document it does not accept is refused for the DOCUMENT even when
    // the resource it addresses is one the store holds nothing for — which would otherwise
    // be answered first.
    let unknown_core_network = "core-network-0000000000000beef";
    let unknown_global_network = "global-network-0000000000000beef";
    for case in refused_documents() {
        let (status, body) = emulator
            .post(
                &format!("/core-networks/{unknown_core_network}/core-network-policy"),
                json!({ "PolicyDocument": case.document }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: PutCoreNetworkPolicy expected 400, got {status} with {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "CoreNetworkPolicyException",
            "case {:?}: the document is judged before the core network is looked up, got {body}",
            case.name
        );
        assert_eq!(
            body["Errors"][0]["ErrorCode"],
            json!(case.error_code),
            "case {:?}: expected the error code {}, got {body}",
            case.name,
            case.error_code
        );

        let (status, body) = emulator
            .post(
                "/core-networks",
                json!({
                    "GlobalNetworkId": unknown_global_network,
                    "PolicyDocument": case.document,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: CreateCoreNetwork expected 400, got {status} with {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "CoreNetworkPolicyException",
            "case {:?}: the document is judged before the global network is looked up, got {body}",
            case.name
        );
        assert_eq!(
            body["Errors"][0]["ErrorCode"],
            json!(case.error_code),
            "case {:?}: expected the error code {}, got {body}",
            case.name,
            case.error_code
        );
    }

    // A document this service DOES accept, on a core network the store holds nothing for,
    // is answered for the missing core network: judging the document first refuses nothing
    // a valid document is entitled to.
    let (status, body) = emulator
        .post(
            &format!("/core-networks/{unknown_core_network}/core-network-policy"),
            json!({ "PolicyDocument": baseline_document().to_string() }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case an accepted document on an unknown core network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case an accepted document on an unknown core network: expected the not-found, got {body}"
    );
}

struct History {
    name: &'static str,
    document: Value,
    /// The topology the core network whose CREATE carried the row's document reports.
    created: Value,
    /// The core network the row's document is put, executed, restored and executed on.
    core_network_id: String,
    /// That core network's topology under the baseline document.
    baseline: Value,
    /// The change set of the version the put minted.
    put_changes: Value,
}

#[tokio::test]
async fn a_policy_document_is_expanded_once_and_every_path_reports_that_expansion() {
    let emulator = Emulator::new();
    let baseline = baseline_document();
    let mut histories: Vec<History> = Vec::new();

    // The CREATE path: a core network whose create carried the row's document reports the
    // topology that document describes, and a second core network starts from the baseline
    // document with the row's document submitted on top of it.
    for case in document_cases() {
        let created_on = core_network_with(
            &emulator,
            &format!("{} (created with it)", case.name),
            &case.document,
        )
        .await;
        let created = reported_topology(&emulator, case.name, &created_on).await;
        assert!(
            created[0].as_array().is_some_and(|edges| !edges.is_empty()),
            "case {:?}: the create's document configures edges, got {created}",
            case.name
        );

        let core_network_id =
            core_network_with(&emulator, &format!("{} (baseline)", case.name), &baseline).await;
        let under_baseline = reported_topology(&emulator, case.name, &core_network_id).await;
        assert_ne!(
            created, under_baseline,
            "case {:?}: the row's topology must differ from the baseline's, got {created}",
            case.name
        );

        let version = put_policy(&emulator, case.name, &core_network_id, &case.document).await;
        assert_eq!(
            version, 2,
            "case {:?}: the put appends to the create's version, got version {version}",
            case.name
        );
        let put_changes = change_set(&emulator, case.name, &core_network_id, 2).await;

        histories.push(History {
            name: case.name,
            document: case.document,
            created,
            core_network_id,
            baseline: under_baseline,
            put_changes,
        });
    }

    // One wait for the change-set generation the clock owes every version put above.
    tokio::time::sleep(PAST_SETTLE).await;

    // The PUT path: executing that version's change set puts its document in force, and the
    // topology reported is the one the create of the same document reports.
    for history in &histories {
        execute(&emulator, history.name, &history.core_network_id, 2).await;
        let after_put = reported_topology(&emulator, history.name, &history.core_network_id).await;
        assert_eq!(
            after_put, history.created,
            "case {:?}: a put then executed must report the topology its create reports, got {after_put}",
            history.name
        );
        // A read derives nothing: the second read answers what the first did.
        let read_again = reported_topology(&emulator, history.name, &history.core_network_id).await;
        assert_eq!(
            read_again, after_put,
            "case {:?}: reading a core network twice must report one topology, got {read_again}",
            history.name
        );

        let version = put_policy(
            &emulator,
            history.name,
            &history.core_network_id,
            &baseline_document(),
        )
        .await;
        assert_eq!(
            version, 3,
            "case {:?}: the baseline is put back as version 3, got version {version}",
            history.name
        );
    }

    tokio::time::sleep(PAST_SETTLE).await;

    // Back to the baseline, so the restore below is a real change again; then the RESTORE
    // path re-submits the version the put stored, which carries the expansion of the same
    // document — so its change set against the same policy in force is the same change set.
    for history in &histories {
        execute(&emulator, history.name, &history.core_network_id, 3).await;
        let back = reported_topology(&emulator, history.name, &history.core_network_id).await;
        assert_eq!(
            back, history.baseline,
            "case {:?}: putting the baseline back must report the baseline topology, got {back}",
            history.name
        );

        let (version, document) =
            restore_policy(&emulator, history.name, &history.core_network_id, 2).await;
        assert_eq!(
            version, 4,
            "case {:?}: the restore appends version 4, got version {version}",
            history.name
        );
        assert_eq!(
            document,
            json!(history.document.to_string()),
            "case {:?}: the restored version must carry the document it was stored with",
            history.name
        );
        let restored_changes =
            change_set(&emulator, history.name, &history.core_network_id, 4).await;
        assert_eq!(
            restored_changes, history.put_changes,
            "case {:?}: the restored version's change set must be the put's, both being the baseline against the same document",
            history.name
        );
    }

    tokio::time::sleep(PAST_SETTLE).await;

    // Executing the restored version puts the same document in force once more, through a
    // third path, and the topology reported is still the one the document describes.
    for history in &histories {
        execute(&emulator, history.name, &history.core_network_id, 4).await;
        let after_restore =
            reported_topology(&emulator, history.name, &history.core_network_id).await;
        assert_eq!(
            after_restore, history.created,
            "case {:?}: a restored version then executed must report the same topology, got {after_restore}",
            history.name
        );
    }
}
