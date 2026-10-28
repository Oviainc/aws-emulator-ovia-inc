// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE version-archival rule both Firewall Manager
//! list writers follow — `PutAppsList` for an applications list and
//! `PutProtocolsList` for a protocols list — now that both apply it from the
//! store's single copy ([`state::archive_superseded_list_version`]).
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: an update archives the array it
//! supersedes under the version number that array held — one past what the
//! archive already names, the create being version 1 — and archives it only
//! while that number still fits the map's modelled key
//! (`com.amazonaws.fms#PreviousListVersion`, pattern `^\d{1,2}$`, so at most
//! [`state::MAX_PREVIOUS_LIST_VERSION`]).
//!
//! The table is two-dimensional, and that is what shows the rule is shared
//! rather than hand-copied: the rows differ in how many successive updates a
//! list takes (one, a few, and two past the cap) and every row is asserted on
//! BOTH list kinds, whose archived element types differ (an `App` structure
//! array and a protocol string array).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_fms_awsfms_20180101::state;

// The wire harness and the administrator seeding live in one place for this test
// target, so no two suites can drift in how a request is signed or the gate is
// opened.
use crate::administrator_gated_operations::associate_admin;
use crate::policy_lifecycle::{call, emulator_with_engine};

/// The applications array version `version` of a list holds: a DISTINCT array
/// per version, so the archive's values say which version each of its keys
/// names.
///
/// `Protocol` is `com.amazonaws.fms#Protocol` and `Port` is
/// `com.amazonaws.fms#IPPortNumber` (published range 0..65535).
fn apps_version(version: usize) -> Value {
    json!([{ "AppName": format!("app-{version}"), "Protocol": "tcp", "Port": 443 }])
}

/// The protocols array version `version` of a list holds, distinct per version
/// the same way. The entry is an IANA protocol NUMBER, the form
/// `com.amazonaws.fms#Protocol` documents beside the protocol keywords.
fn protocols_version(version: usize) -> Value {
    json!([version.to_string()])
}

/// One of the two list kinds the archival rule covers.
///
/// `document` is the member the list document hangs under in both the request
/// and the response AND the member inside it that carries the array — the model
/// spells both `AppsList` for one kind and `ProtocolsList` for the other.
struct Kind {
    /// Names the row in every assertion message.
    name: &'static str,
    writer: &'static str,
    reader: &'static str,
    document: &'static str,
    archive: &'static str,
    /// The array version `n` of a list of this kind holds.
    version: fn(usize) -> Value,
}

/// The two list kinds, i.e. the two former copies of the rule.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            name: "applications list",
            writer: "PutAppsList",
            reader: "GetAppsList",
            document: "AppsList",
            archive: "PreviousAppsList",
            version: apps_version,
        },
        Kind {
            name: "protocols list",
            writer: "PutProtocolsList",
            reader: "GetProtocolsList",
            document: "ProtocolsList",
            archive: "PreviousProtocolsList",
            version: protocols_version,
        },
    ]
}

/// One `Put…List` request body: a create when `list_id` is `None`, an update
/// carrying the list's current token when it is `Some`.
fn write_body(
    kind: &Kind,
    list_id: Option<&str>,
    token: Option<&str>,
    list_name: &str,
    array: Value,
) -> Value {
    let mut document = serde_json::Map::new();
    document.insert("ListName".to_string(), json!(list_name));
    document.insert(kind.document.to_string(), array);
    if let Some(list_id) = list_id {
        document.insert("ListId".to_string(), json!(list_id));
    }
    if let Some(token) = token {
        document.insert("ListUpdateToken".to_string(), json!(token));
    }
    let mut body = serde_json::Map::new();
    body.insert(kind.document.to_string(), Value::Object(document));
    Value::Object(body)
}

/// The archive a list of `kind` must report after `updates` successive updates:
/// one key per superseded version, numbered from 1, up to the cap the modelled
/// key can name.
fn expected_archive(kind: &Kind, updates: usize) -> Value {
    let mut archive = serde_json::Map::new();
    for version in 1..=updates.min(state::MAX_PREVIOUS_LIST_VERSION) {
        archive.insert(version.to_string(), (kind.version)(version));
    }
    Value::Object(archive)
}

/// Create a list of `kind` and update it `updates` times, each update sending
/// the array of the NEXT version and asserting the archive the write reports.
/// Answers the last write's response body.
async fn create_and_update(router: &MultiServiceRouter, kind: &Kind, updates: usize) -> Value {
    let case = format!("{} after {updates} update(s)", kind.name);
    let list_name = format!("archive-{updates}");
    let (status, mut body) = call(
        router,
        kind.writer,
        &write_body(kind, None, None, &list_name, (kind.version)(1)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to answer 200, got {status} with {body}"
    );
    assert!(
        body[kind.document]
            .get(kind.archive)
            .is_none_or(|archive| *archive == json!({})),
        "case {case:?}: a list created once must report no archived version, got {body}"
    );
    let list_id = body[kind.document]["ListId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create must mint a ListId, got {body}"))
        .to_string();

    for superseded in 1..=updates {
        let token = body[kind.document]["ListUpdateToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: every write must report a ListUpdateToken, got {body}")
            })
            .to_string();
        let (status, updated) = call(
            router,
            kind.writer,
            &write_body(
                kind,
                Some(&list_id),
                Some(&token),
                &list_name,
                (kind.version)(superseded + 1),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected update {superseded} to answer 200, got {status} with {updated}"
        );
        assert_eq!(
            updated[kind.document][kind.archive],
            expected_archive(kind, superseded),
            "case {case:?}: after {superseded} update(s) the archive must hold every superseded \
             version under its own number, got {}",
            updated[kind.document][kind.archive]
        );
        body = updated;
    }
    body
}

/// The rule, on both kinds: each update archives the version it supersedes, and
/// the reader reports the same archive the writer returned.
#[tokio::test]
async fn each_update_archives_the_version_it_supersedes() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    for kind in kinds() {
        for updates in [1_usize, 2, 5] {
            let case = format!("{} after {updates} update(s)", kind.name);
            let body = create_and_update(&router, &kind, updates).await;
            assert_eq!(
                body[kind.document][kind.archive],
                expected_archive(&kind, updates),
                "case {case:?}: unexpected archived history, got {body}"
            );
            assert_eq!(
                body[kind.document][kind.document],
                (kind.version)(updates + 1),
                "case {case:?}: the current array must be the one last written, got {body}"
            );

            // The reader agrees with the writer: the archive is persisted state,
            // not something the write made up for its own response.
            let list_id = body[kind.document]["ListId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (status, got) = call(&router, kind.reader, &json!({ "ListId": list_id })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {} to answer 200, got {status} with {got}",
                kind.reader
            );
            assert_eq!(
                got, body,
                "case {case:?}: the reader must report what the writer wrote, got {got}"
            );
        }
    }
}

/// The cap half of the rule, on both kinds: a list updated more times than the
/// modelled two-digit key can number keeps the versions the map can name and
/// archives no further one, so no key outside the pattern is ever emitted.
#[tokio::test]
async fn the_archive_stops_at_the_modelled_two_digit_version_cap() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    // Two updates PAST the cap, so both the first version the map cannot name
    // and the one after it are observed.
    let updates = state::MAX_PREVIOUS_LIST_VERSION + 2;
    for kind in kinds() {
        let case = format!("{} after {updates} update(s)", kind.name);
        let body = create_and_update(&router, &kind, updates).await;
        let archive = body[kind.document][kind.archive]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected an archive map, got {body}"));
        assert_eq!(
            archive.len(),
            state::MAX_PREVIOUS_LIST_VERSION,
            "case {case:?}: the archive must keep the {} versions the modelled key can name, got \
             {} of them",
            state::MAX_PREVIOUS_LIST_VERSION,
            archive.len()
        );
        for key in archive.keys() {
            assert!(
                (1..=2).contains(&key.len()) && key.chars().all(|c| c.is_ascii_digit()),
                "case {case:?}: every archived version must be a modelled PreviousListVersion \
                 (one or two digits), got {key:?}"
            );
        }
        assert_eq!(
            body[kind.document][kind.archive],
            expected_archive(&kind, updates),
            "case {case:?}: unexpected archived history, got {body}"
        );
        assert_eq!(
            body[kind.document][kind.document],
            (kind.version)(updates + 1),
            "case {case:?}: an update past the cap must still store the array it sent, got {body}"
        );
    }
}
